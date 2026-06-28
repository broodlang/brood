# Dev log

Chronological record of work sessions. Newest at the bottom.

## How to navigate

The session history is split so this file stays loadable:
- **This file** = the **complete digest** (every session, one line, by date) plus
  the **most recent day in full** at the bottom, where new entries get appended.
  The digest *is* the record of the timeline; the load-bearing *why* of any change
  lives in its `## ADR-NNN` ([decisions.md](decisions.md)) or topic doc, not in a
  blow-by-blow session log.
- **[devlog-archive.md](archive/devlog-archive.md)** = full verbatim text of the
  **early** sessions (through 2026-05), kept for reference. Later sessions were
  compacted into the digest above (full text recoverable via git if ever needed).

You rarely read either top to bottom. For the *current* state of something, prefer
the topic doc (see [README.md](README.md)) or the relevant `## ADR-NNN` in
[decisions.md](decisions.md). Use the digest to place a change in time; for an early
session's full text, find its `## YYYY-MM-DD — …` header in the archive.

**Maintenance:** keep this lean. Append a new session as a **full entry** under
"Recent"; once it's older than a day or two, condense it to its **one-line digest
entry** and drop the verbose text (don't grow the archive). Prune anything that
won't help future work — the ADRs and topic docs carry the durable rationale.

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

Every session, oldest first. Early sessions' full text is in
[devlog-archive.md](archive/devlog-archive.md); the latest day is in "Recent" below.

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
- **2026-06-01** — Nodes form a transitive cluster mesh (ADR-088)
- **2026-06-01** — Resilient `ui-run`: let-it-crash at the render loop (M3)
- **2026-06-01** — Node-link channel encryption: a Noise-style session (ADR-089, M4)
- **2026-06-01** — M4 daemon/serving layer: serve a ui-run app to remote frontends (ADR-090)
- **2026-06-01** — RUNTIME-region GC, Stage 1: solidify the single-process collector (ADR-091)
- **2026-06-01** — `nest grammar`: editor grammars generated from the language (ADR-092) + a VS Code extension
- **2026-06-01** — tree-sitter-brood: a real parser grammar (+ `nest grammar tree-sitter`)
- **2026-06-02** — GUI key fix: Shift survives Alt/Ctrl punctuation chords
- **2026-06-02** — GUI window titles: `gui-open` title arg + `gui-title!`
- **2026-06-03** — gui-open: optional initial window size
- **2026-06-03** — gui: release a held mouse button on cursor-leave / focus-loss
- **2026-06-03** — gui: gui-icon! sets a window's taskbar / title-bar icon
- **2026-06-03** — nest release: functional, repeatable `--target` via a local runtime cache
- **2026-06-03** — gc: harden reset_local_to against a collection inside the bracket
- **2026-06-04** — gc: rewrite the `remembered` write-barrier set across a major collection
- **2026-06-04** — vm: register the tail-call arm before push_frame (RUNTIME use-after-GC)
- **2026-06-04** — builtins: guard `span-runs` against an i64-overflow host panic
- **2026-06-04** — dist: bound the per-link writer queue (remote-controlled OOM)
- **2026-06-04** — wire: cap `prealloc` against element-size amplification
- **2026-06-04** — builtins: cap `to-fixed` decimal count
- **2026-06-04** — heap: delete the dead mark-sweep collector
- **2026-06-04** — scheduler: assign_worker indexes by WORKERS.len()
- **2026-06-04** — gc: de-dup the write-barrier remembered set
- **2026-06-04** — lsp: resolve_in_source stops interning transient identifiers
- **2026-06-04** — kernel-audit hardening batch (the low-impact tail)
- **2026-06-04** — review pass over the kernel-audit series
- **2026-06-06** — whole-kernel review sweep: review everything, fix everything
- **2026-06-06** — std/ review sweep: the Brood-language counterpart
- **2026-06-06** — reducible lazy range (Value::Range)
- **2026-06-06** — ADR-096: VM perf round as the JIT runway (plan)
- **2026-06-06** — ADR-096 round 1 shipped: ICs, wider prims, rooting skip, exec split
- **2026-06-07** — ADR-096 round 2 (item 6): direct letrec self-recursion on the VM
- **2026-06-07** — ADR-098: shrink the core (drop `lambda`/`let*`; `defmacro` → macro)
- **2026-06-07** — ADR-099: `proc/gen` becomes a real gen_server
- **2026-06-07** — scheduler: sticky `:kill` + busy-aware spawn placement
- **2026-06-07** — fix: flaky `unbound` under load was test-isolation, not a core race
- **2026-06-07** — VM bench harness, perf-stats pass, apply-unfolding in dispatch
- **2026-06-07** — scheduler: fresh-only work-stealing + the full-migration design (ADR-100)
- **2026-06-07** — bytecode stepping engine, Stage 1 (the §7 endgame begins)
- **2026-06-07** — bytecode engine Stages 2–4: calls, closures, and the explicit frame stack
- **2026-06-07** — bytecode Stage 5: call-site IC + bytecode is now the default engine
- **2026-06-08** — corosensei removal §8.4 step 1: state-capture machinery (flag-gated)
- **2026-06-08** — corosensei removal §8.4 step 2: dual-mode run_one + live process migration
- **2026-06-08** — corosensei removal §8.4 steps 3-flip + 4: corosensei is gone
- **2026-06-08** — stdlib expansion — path, system, crypto, agent, enum extras
- **2026-06-08** — HMAC primitives: ~200x speedup for hmac-sha256/sha1/sha512
- **2026-06-08** — JIT Stage 1 landed (tier-1 template JIT via Cranelift, ADR-101)
- **2026-06-08** — JIT: compile on a background thread (scheduler-starvation fix)
- **2026-06-09** — JIT Stage 1.5/2: fire on real fused code, + 4 correctness fixes
- **2026-06-09** — JIT tier-2 foundation: hybrid operand model (handles in roots)
- **2026-06-09** — JIT: cons / car / cdr land (the JIT fires on list code)
- **2026-06-10** — Kernel review: two bugs fixed (timer wakeup, prim2 de-opt) + cleanup
- **2026-06-10** — JIT tier-2: Brood→Brood calls (non-tail + tail-call TCO)
- **2026-06-13** — Persistent child processes: `proc-spawn`/`proc-send`/`proc-close` (ADR-104)
- **2026-06-14** — JIT: two small codegen wins from a cross-language benchmark audit
- **2026-06-14** — JIT: top-level-lambda promotion (pipeline ~4.1×, matmul ~2.2×)
- **2026-06-14** — `proc-spawn` options map: `:cwd` + `:env` (ADR-104 update)
- **2026-06-14** — LSP: hover + goto on `defmodule` `:use`/`:alias`/`:implements` clauses
- **2026-06-14** — LSP document links + variadic-callback arity check; verified defdyn isn't statically pinned
- **2026-06-14** — JIT: lower `and`/`or` (mandelbrot ~5.3×) + fix two promotion-exposed regressions
- **2026-06-14** — LSP: selection range, context-aware module completion, two more code actions (+ a doc-link bug fix)
- **2026-06-14** — fix two cross-node regressions from the inline-lambda JIT promotion (dfa4f67)
- **2026-06-14** — atomic spawn-link: a real supervisor bug behind a flaky test
- **2026-06-14** — telemetry: an Erlang-shaped `:telemetry`, inline dispatch (ADR-106)
- **2026-06-14** — `table`: an in-memory shared store (Brood's ETS, ADR-107)
- **2026-06-14** — telemetry: reverse to a listener process so a handler can never crash the emitter (ADR-106)
- **2026-06-14** — `lambda`/`let*` are real synonyms; three checker false-positives fixed
- **2026-06-14** — JIT matmul LICM: hoist an invariant vector's element base out of the loop
- **2026-06-14** — Checker false-positive sweep (bucket A): transient args, unexpandable macros, dynamic-namespace refs
- **2026-06-14** — Structured types, fifth slice: element flow through the rest of the sequence library
- **2026-06-15** — scheduler: floor the worker pool at 2 (a single worker can't drain a dirty-block)
- **2026-06-15** — scheduler: on-demand dirty-scheduler growth (the complete native-nested-receive fix)
- **2026-06-15** — GradualTy gets its first consumer: gradual-assignment checking of `(def x …)` vs `(sig x T)`
- **2026-06-15** — Gradual typing, slice 2: return-type checking + declared globals in value position
- **2026-06-15** — JIT LICM, the global lever: hoist an invariant *global* vector's base + epoch-guard the back-edge
- **2026-06-15** — Gradual typing, slice 3: precise sig-param returns (the first non-disjoint catch)
- **2026-06-15** — Session close: type-checker hardening + gradual typing, and what we learned
- **2026-06-15** — Lazy seq-views: fusing map/filter pipelines, opt-in (compute-frontier lever 3c)
- **2026-06-15** — Remove user-facing transients: Brood data is immutable, full stop (only Table is mutable)
- **2026-06-15** — mimalloc backend: spend memory for speed (Brood is for long-running apps)
- **2026-06-16** — Call-path + escape-analysis perf round (BEAM-grounded), and what didn't work
- **2026-06-16** — JIT: liveness-driven multi-slot handle spill (the inlining prerequisite)
- **2026-06-17** — JIT Phase B: recursive self-inlining (the fib lever) — ~1.7× on fib
- **2026-06-17** — Self-inliner shelved default-OFF; it's net-negative globally (the lesson)
- **2026-06-17** — Allocation levers measured NEUTRAL; the lever is frame representation
- **2026-06-17** — Frame-rep prototype: per-call protocol cost is NOT the frame ops (measured)
- **2026-06-17** — Operand-stack-in-registers measured NEUTRAL; the interpreter micro-opt approach is exhausted
- **2026-06-17** — Weakness-hunt: isolated CHAMP `assoc` (~2.2µs) as the map-perf target; lever is FBIP reuse
- **2026-06-17** — Native inline `nth` measured NEUTRAL; 8 experiments converge: per-call dispatch is THE cost, inlining is the only lever
- **2026-06-17** — BREAKTHROUGH: inlining confirmed (fib 1.55×, pfib 1.6×); per-engine frame sizing is the last blocker
- **2026-06-17** — Per-engine frame sizing WORKS (fib 1.61×, bintree/nqueens flat); spawn-tiering-contention is the corrected last blocker
- **2026-06-17** — JIT recursive self-inliner ships DEFAULT-ON via two-stage tiering (fib 1.7×, spawn flat) — the campaign's first real perf win
- **2026-06-17** — Correction: inliner must skip heap-touching recursion (fixes a bintree ~15× regression in the shipped inliner)
- **2026-06-18** — 8-byte Value rep: Stage 0 complete (accessor-first migration)
- **2026-06-18** — 8-byte Value rep: prototyped and REJECTED (NO-GO)
- **2026-06-18** — Track B kickoff: kill the per-call JIT dispatch protocol (Technique A)
- **2026-06-18** — Track B / Technique A increment 1: in-IR epoch-guarded call fast-link (shipped, ~20% on fib)
- **2026-06-19** — Track B / Technique A increment 2: in-IR frame setup — implemented, REGRESSED, reverted (NO-GO)
- **2026-06-19** — JIT: raw-load the global epoch instead of a per-iteration FFI (~21% on `loop`)
- **2026-06-19** — `map-int-add` + JIT GC safepoint: `wordcount` 810 → 470 ms
- **2026-06-19** — nil?/pair?/empty? as native builtins: bintree −37%, nqueens −41%
- **2026-06-19** — JIT: lift the chunk_walks_structure gate; fix Prim2SlotInt VectorRef: bintree −50%
- **2026-06-19** — JIT: PrimOp1::IsEmpty — nqueens −48%
- **2026-06-19** — JIT: register-carry for loop-carried Int params — loop −37%, collatz −11%
- **2026-06-20** — JIT: float register-carry + F64 SSA value cache; mandelbrot −9%
- **2026-06-20** — max/min as PrimOp2 native + cranelift `select`; collatz −66%
- **2026-06-20** — JIT: inline `first`/`rest` slab reads; nqueens −16%
- **2026-06-20** — %range-reduce tight i64 loop; reduce −80%
- **2026-06-22** — REPL: C-j accepts the line (typed-ahead `\n` at startup didn't submit)
- **2026-06-24** — JIT fast path: stale LOCAL handle after GC in `dispatch`'s `_ =>` arm
- **2026-06-28** — GC cost study + ADR-114: keep the moving collector, fix stale handles with JIT stack maps (not mark-sweep)
- **2026-06-28** — Raw-byte crypto/encoding + binary I/O (`proc-set-binary`/`slurp-bytes`/binary `http-read-request`); fixed the `remote-spawn` spawn-footgun sibling; test hardening (no flaky timeouts / skips / ignores) + devlog/ADR compaction
- **2026-06-28** — `make install` now uses a new `release-fast` profile (stripped, no LTO) instead of fat-LTO `release-lean` — builds in a fraction of the time (bigger binary, ~36 MB vs ~10 MB, is the trade-off; thin LTO measured to give no size win here so it's not used). `release-lean` stays for `nest release`'s shippable runtime. Also fixed `make help` (was printing "Makefile" for every name once `config.mk` existed)

---

## Recent — full entries

The latest day in full; older sessions' full text is in
[devlog-archive.md](archive/devlog-archive.md). Append new sessions below (newest last).

## 2026-06-28 — GC cost study + ADR-114: keep the moving collector, fix stale handles with JIT stack maps (not mark-sweep)

Prompted by the question *"immutability + process isolation should make GC easy
— are we over-complicating it?"* — a good question, pinned down with data.

**What the invariants already buy (and we cash in).** Immutability ⇒ old never
points to young ⇒ **no data write barrier** (sole remembered set is the `def`/env
rebind, ADR-013). Isolation ⇒ per-process collection, no stop-the-world,
free-on-death. That part is genuinely simple.

**What's left isn't from mutation.** The epoch stamps / poison bits / per-deref
tripwire / `BROOD_GC_VERIFY` verifier all exist solely to catch the *stale-handle*
class a **moving** collector creates — the bug #2 family (a JIT-staged LOCAL
handle held across a collection). Immutable data is *nearly* acyclic, so a
**non-moving** mark-sweep heap would erase that class by construction (handles
never move). Question: what throughput does that cost?

**Measured it** (A/B GC-on vs `BROOD_GC_FLOOR=500M` → 0 collections; clean
`--release --features jit`; min of 6; archived
`docs/benchmarks/2026-06-28T09-27-19Z-gc-cost.md`):

| workload | survivor | GC-on | GC-off | copying GC effect |
|---|---|---|---|---|
| fib 32 (compute) | — (0 collections) | 0.08s | 0.08s | none |
| listsum (14% surv) | 14% | **0.93s** | 1.57s | **−40%: copying *faster*** (compaction → cache-hot) |
| bintree (60% surv) | 60% | 8.64s | **5.42s** | **+37%: copying is the cost** (copies live trees) |

Findings: (1) compaction is sometimes a *net win* (listsum) — mark-sweep forfeits
it and adds a free-list-vs-bump alloc tax on every workload; (2) copying only
hurts on high-survivor allocation-pathological code (bintree, the GC-stress
benchmark). **Throughput is a wash** → doesn't decide it.

**ADR-114 (accepted).** First draft recommended adding JIT **GC stack maps** —
then I read the JIT↔GC code and that premise was *wrong*: Brood keeps `Value` as
a 16-byte enum (NaN-boxing declined), so a `Value` **never rides in a register**;
JIT'd code keeps all live handles in `Heap::roots` (collector-scanned) and spills
every `Op::Handle` to a GC-visible frame slot before any call (`jit_lower.rs`
~L1981). The ABI doc literally calls the no-stack-map problem "sidestepped." So
stack maps would be **pure redundancy**. The real bug class (bug #2 family,
`dbf134a`/`e000652`) is **Rust dispatch glue** caching a LOCAL `Value`/`EnvId`
(`cur_argv`, a `fast` IC link) across a JIT safepoint and reading it stale instead
of re-reading from `roots`. Audit: only `dispatch` holds a Rust-local across
`jit_tier`, and its post-call arms already re-read from roots; `vm_run_bc`'s caller
is roots-only (immune). Hardened the one residual unverified spot — the rest-arm
`cur_argv` fallback — with a `debug_assert!(heap.dbg_value_stale(v).is_none())`.
Decision: keep the moving collector, **don't build stack maps**, harden the
spill-to-roots discipline. Mark-sweep stays the simplicity-first fallback.
Validated: GC/JIT/dispatch/tail Rust tests 33/33 (debug-assertions armed), brood
suite green, bintree/listsum/nqueens + a rest-arg workload clean under
`BROOD_GC_STRESS=1` (assert never fired).

**Also (lag diagnostics).** The format/`JumpIfFalse` fix (4345d34) shipped; the
gameplay lag still reproduces with no `[stall]` line, so the pause is neither
minor-GC nor RUNTIME-compaction. Broadened `BROOD_STALL_MS`: a **quantum** guard
in `scheduler::run_one` (catches a slow blocking builtin / long eval inside a
green-process quantum, with pid) and a **gui-paint** guard in `gui.rs`
`RedrawRequested` (the native render thread the scheduler guards can't see). Built
a GUI-capable `nest` (`--features brood/jit,brood/gui`) with both; next freeze
should name itself.

## 2026-06-28 — Raw-byte crypto/encoding + binary I/O; spawn-footgun siblings; test/doc hardening

Closing the `store` (Postgres driver) findings (`docs/store-driver-findings-2026-06-28.md`).

**Raw-byte crypto/encoding (findings 2/3/4/5/6).** The stdlib's hash/crypto/encoding
were string/hex/UTF-8-oriented, so SCRAM-SHA-256 forced ~150 lines of pure-Brood
reimplementation at ~2s/connection. Added `%sha256-raw`/`-sha1`/`-sha384`/`-sha512`/
`-md5` (byte vec → digest byte vec), `%hmac-sha256-raw`/`-sha1`/`-sha512` (byte-vec
key+msg → byte vec), and `%pbkdf2-sha256-bytes` (byte-vec password+salt, microseconds
vs ~2s) with Brood wrappers; `crypto/pbkdf2` now coerces a string-or-bytes
password/salt so a binary (base64-decoded) salt round-trips faithfully. base64/hex
gained pure-Brood byte-vector variants (`*-encode-bytes`/`*-decode-bytes` + URL-safe)
via an `:invalid` decode sentinel (so empty-valid stays distinct from failure). New
`tests/scram_bytes_test.blsp` builds the whole SCRAM client-key chain over the new
layer against RFC 7677 §3 (and across processes); FIPS / RFC 4231 known-answer
vectors added to the hash/crypto/encoding tests.

**Spawn-footgun siblings (finding 1).** Fixed a stale `chaos_test` case that encoded
the pre-`1a63eb7` spawn no-op. A follow-up audit (three parallel sweeps) found one
real sibling: `remote-spawn`/`remote-spawn-sync` wrapped their body in `(fn () …)`
*unconditionally*, missing the `spawn--thunk-form?` guard — so `(remote-spawn node
(fn () body))` double-wrapped and the receiver ran nothing. Fixed with the same
guard; `tests/remote_spawn_test.blsp` covers both forms. Every other body/thunk macro
was cleared (they *return* the body value rather than *calling* a produced thunk). The
once-"known segfault" `promote_env` case turned out already fixed (cycle detection in
`closure_to_message`); converted its stale comment into two real regression tests.

**Binary I/O (finding 7 instances).** Keeping the Latin-1 byte-string carrier:
`proc-set-binary` mirrors `tcp-set-binary` for subprocess stdio (byte-faithful both
ways), closing the socket/subprocess asymmetry; `slurp-bytes` is a byte-faithful file
read → byte vector, and `package--sha256-file` now hashes bytes (`%sha256-bytes ∘
slurp-bytes`) so a binary asset hashes instead of throwing — *identical* hash for text
files, no lockfile churn; `http-read-request` reads in binary mode (exact framing +
byte-counted `Content-Length`) then restores text mode before the response path.

**Test hardening.** Bumped flaky cross-process *expected-message* receive timeouts to
the suite's proven-stable 5000ms (absence-checks and deliberate timeout-fires left
intact); un-`#[ignore]`d `mem_limit` with a bounded runaway (safe unattended); fenced
the `root_scope` doc example as `text`. `make test`: 629 passed, 0 skipped, 0 timed
out; doctests 1 passed, 0 ignored.

## 2026-06-28 — Dependency refresh to latest stable + a docs-driven crate audit

**Latest-stable bumps.** Moved the workspace onto current stable for every dep that
had one: `rustls` 0.23.41, `rcgen` 0.14.8 (`CertifiedKey.key_pair`→`signing_key`),
`lsp-server` 0.8.0 (no code change — the only break, `Message::write(&self)`, isn't
called), `rodio` 0.22.2 (rewrote the audio backend onto `DeviceSinkBuilder`/`mixer`;
`convert_samples` is gone; added the `playback` feature since `default-features=false`
now drops it), `glow` 0.17 (additive — no migration), and `cranelift` 0.132→0.133
(the big one: `MemFlags` became an interned `u16` index, so all 52 JIT `load`/`store`
sites moved to `MemFlagsData::new()`). The crypto-stack **dedup** stays parked — it
needs `chacha20poly1305 0.11` to ship stable (only an rc, stalled since Feb).

**Audit + fixes.** Swept every direct dep against its docs (8 parallel readers). Applied:
`smallvec` `union` feature (shrinks the hot `MapNode` + per-call argv/env vecs; sound —
all element types are `Copy`); the JIT's own heap loads/stores now use
`MemFlagsData::trusted()` (the ~40 provably-aligned/non-trapping accesses; the 6 scalar
`bitcast`s keep `new()`); a borrowing `expect_rope_ref` for the 7 read-only rope builtins
(skips an `Arc`-node clone on the editor render path); an x25519 `was_contributory()`
check in the node handshake (makes the low-order-key guard explicit rather than emergent
from the cookie-MAC); `rcgen` switched to `aws_lc_rs` so **`ring` is dropped** (one crypto
provider, not two); and the dead `glutin-winit` dep removed (gui_gpu builds the GL surface
via raw-window-handle directly).

**This-round cleanups.** `tree-sitter-parse` now projects `:error`/`:missing` recovery
state into the CST (only when set; +3 tests); the `%chacha20-encrypt` Rust docstring +
`PRIMITIVE_DOCS` got the nonce-reuse warning (the Brood side already had it + a
`random-nonce` helper); dropped `num-integer` by computing even bignum division with
`BigInt`'s inherent `%`/`/`; `glow` loader uses `from_loader_function_cstr` (no `CString`
round-trip); `nest grammar`'s target is now a clap `ValueEnum` (lists choices + a
formatted error instead of a hand-rolled `exit(2)`); fixed the stale `Content-Length`
comment in `nest/Cargo.toml` (the MCP transport is newline-delimited). And
`bigint_cmp_float` (the `value_cmp` bignum-vs-float fallback) now compares
*exactly* via `BigDecimal` instead of rounding the bignum to f64 — so a bignum
inside f64's range but not exactly representable (e.g. 2^70±1 vs the f64 2^70) is
ordered correctly rather than called equal (+3 Rust unit tests). Suite green
throughout (2497 passed) across default + `jit` + `gui-gpu`.

## 2026-06-28 — Exact Decimal/Float ordering, tree-sitter incremental parse, damage-only GUI present

Follow-on round taking three audit items to completion.

**Exact Decimal-vs-Float ordering.** Generalised the bignum-vs-float fix into a
shared `bigdecimal_cmp_float` and applied it to the `value_cmp` Decimal/Float arms,
which had the same `to_f64()` precision loss — sort ordering of mixed decimals and
floats (e.g. the decimal `0.1` vs the f64 `0.1`, which is slightly larger) is now
exact, not rounded-then-compared (+1 unit test). Ordering is exact even though the
arithmetic tower still does deliberate float contagion — distinct concerns.

**Tree-sitter incremental parsing (ROADMAP §C).** New `tree-sitter-reparse key
source lang` keyed by a buffer id: caches the last `(source, tree)` and re-uses it
via `Tree::edit` + `parse(.., Some(old_tree))`, so only the changed region is
re-scanned — what a self-editing editor needs to reparse on each keystroke. The edit
is derived by **diffing** the cached source against the new (longest common prefix +
suffix → one `InputEdit`, snapped to char boundaries), so the editor needn't track
edit ranges. `Parser`s are pooled and `Tree`s cached in global `Mutex` maps (both are
`Send+Sync`; green processes migrate across worker threads, so thread-local wouldn't
hit). `tree-sitter-forget key` evicts on buffer close; the cache is capped. The result
is **identical** to a from-scratch parse — a test asserts incremental == scratch across
insert/delete/append/multibyte/no-op edits (incrementality is pure optimization).

**Damage-only GUI present (softbuffer).** The CPU `gui` backend now declares only the
changed screen region instead of blitting the whole framebuffer — the win for the
typing case the paint-stall trace flagged as present-bound. Correct under
multi-buffering via softbuffer's `Buffer::age()`: the declared damage is the union of
the last `age` frames' changed bboxes (diffed against the last presented pixels).
**Safe by construction** — it narrows only when the buffer age is known and within
recorded history, and full-presents (the prior exact behaviour) on any doubt or
resize, so a wrong-damage corruption is impossible. Validated on a live display and
made the **default**; `BROOD_GUI_DAMAGE=0` is the opt-out escape hatch. Builds clean
across default/`jit`/`gui`/`gui-gpu`; suite 2499 passed.

## 2026-06-28 — Tech-debt sweep: box `LispError`, zero clippy warnings, fatal gate

**`LispError` is now one pointer wide.** It was a 144-byte struct returned by value in
every `LispResult` (355 functions) — both a hot-path cost and 82% of the clippy noise
(`result_large_err` ×479). Made it a newtype over a boxed `LispErrorData` with
`Deref`/`DerefMut`, so **`LispResult` went 144 → 24 bytes** (6× smaller returns on the
common `Ok` path; the box only allocates on the cold error path) with the change fully
contained to `error.rs` — Deref keeps every `e.field` access, the `with_*`/`or_*`
builders, and all 165 `Err(LispError::…)` sites unchanged. Brood suite green (2499);
the only follow-on edits were three test sites that *moved* `e.message` out (now
`.clone()`).

**Clippy: 582 → 0 warnings, and the gate is now fatal.** `cargo clippy --fix` cleared
the mechanical lints; the design-intentional ones got documented crate-level allows
(`too_many_arguments`/`type_complexity` in the evaluator/codegen/render paths;
`result_large_err` locally on cranelift's external error; the cosmetic doc-style lints;
and a small set of style lints whose lint-preferred form is less clear in the kernel's
index-based loops). Substantive fixes landed: boxed the 6 KB `Backend::Gpu` enum variant,
`pub`→`pub(crate)` on `GlWindow::paint` (private type in public API), `SharedBlob::is_empty`,
exact `bigdecimal_cmp_float` reuse, `# Safety` docs on the debug JIT callbacks, and the
unused-var / always-true-assert test nits. `make clippy` now runs `-D warnings` so a new
lint fails the build (docs/allows in `crates/lisp/src/lib.rs`). Also dropped a duplicate
`expect!` macro definition. Earlier in the day: bumped deps to latest stable, the audit
fixes (smallvec `union`, cranelift `MemFlagsData::trusted()`, ropey borrow-not-clone,
x25519 `was_contributory`, rcgen→aws_lc_rs dropping `ring`, removed dead `glutin-winit`),
tree-sitter incremental parse + `:error`/`:missing` CST keys, and the softbuffer damage
present.

## 2026-06-28 — Checker: unused `:use` imports (Pass 4.5) + unused private defns (Pass 4.6)

Two new advisory lints in `check_file`, running after the hygiene pass:

**Unused `:use` imports (Pass 4.5)** — warns when a `(:use mod)` clause contributes
public names that are never referenced in the file's expanded forms. Implementation:
`extract_use_module_names` parses `:use` keywords from the *unexpanded* defmodule header
(the clauses lower away after expansion); `heap.imported_pairs()` (snapshotted during Pass 1
before the import table is restored) provides the bare→qualified mapping; a single
`collect_all_syms` walk over all expanded forms builds the reference set; grouping by module
prefix and checking intersection emits "unused :use import: mod" for strays. Modules that
contributed zero public names (failed require, empty export set) are skipped silently — no
false positives. Added to `walk.rs`: `collect_all_syms` / `collect_syms_into`.

**Unused module-private defns (Pass 4.6)** — warns when a `(def name …)` whose bare segment
contains `--` (the private-name convention, same gate as `%refer`'s refer-all skip) is never
referenced outside its own definition. Implementation: `collect_private_defs` scans the
expanded tree for `(def name …)` with a private bare name; `sym_used_beyond_def` rescans,
skipping the binding-name slot of the def itself (so a self-recursive private fn isn't
flagged) but checking all other forms freely. Warning shows just the bare name (not the
qualified path). Public names are never checked — they may be used by other files. Added to
`walk.rs`: `sym_used_beyond_def`.

7 new tests in `mod tests`: unused `:use` flagged, used `:use` silent, no-`:use` silent,
private unused flagged, private used silent, self-recursive private silent, `_`-prefix not
treated as private.

## 2026-06-28 — Checker: unused let binding lint + goals reframe

**Unused `let` locals** (`crates/lisp/src/types/check/walk.rs`): added
`sym_appears_in` (conservative recursive scan for a symbol in a form) and extended
`check_let` to emit "unused let binding: x" for each bound name that never appears
in its visible scope (subsequent binding RHSs + body; preceding bindings too for
`letrec`). Key design choices:

- **Conservative scan**: counts any occurrence (binder positions, quoted forms) →
  zero false positives at the cost of false negatives for shadowed names.
- **`_`-prefix exemption**: names starting with `_` are silently skipped.
- **Position gate**: compiler-generated `let`s (match/pattern expansion) have no
  reader-assigned source position — `heap.form_pos_only(form)` returns `None`.
  Skipping when `None` correctly exempts pattern variables (`([a b] :vec)` match
  arms) that are unused in the branch body, with zero change to the expansion
  machinery. Found via `type_check_catalog` false-positive in the same run.
- **`let*` is already `let`**: compile pass rewrites `let*` → `let`; lint is free.

**Goals reframe** (`docs/roadmap.md`): replaced "never gates" with the correct
formulation — **gate what is provably local and static; advisory at reload
boundaries**. Globals are `dynamic()` because hot reload works by rebinding them;
static gating on global types would reject valid reloads. The right split: Elixir's
checker for the *interior* of a function/let-scope; Erlang's late binding for globals
and module boundaries.

## 2026-06-28 — Checker: expand curated-sigs table (25 new entries)

Added 25 entries to `CURATED_SIGS` in `crates/lisp/src/types/check/sigs.rs`, covering
the stdlib functions that have branchy/recursive/variadic/rest-param or `apply`-based
bodies that `infer_sig` can't walk:

- **Equality**: `=` / `not=` (multi-arm closures; pins `bool` result so `(+ 1 (= x y))` is caught).
- **String conversion**: `number->string` (`num → str`, tighter domain than the `str` primitive's `any`), `string->symbol` (`str → sym`).
- **String predicates**: `starts-with?`, `ends-with?`, `string-contains?` (`str,str → bool`), `blank?` (`str → bool`).
- **String transforms**: `trim`/`triml`/`trimr` (`str → str`), `replace` (`str,str,str → str`), `string-repeat` (`str,int → str`), `pad-left`/`pad-right` (`str,int → str`), `char-at` (`str,int → str`).
- **String/list conversions**: `string->list` (`str → list`), `list->string` (`seq → str`), `string-codepoints` (`str → vector`), `string-from-codepoints` (`seq → str`).
- **Format**: `format` (`str, &any → str` — catches non-string template arg and flows string result out).
- **Search → int**: `index-of`, `index-where` (`cb1,seq → int`), `string-index-of` (`str,str → int`).

New `curated_equality_and_string_sigs` test covers domain checks (arg-type errors) and
result-type flow (return value used in a numeric or string sink). All 634 suite tests pass.

## 2026-06-28 — GUI: less-TUI refinements (centred remainder, themed bg, line-height, slim scrollbar) + a cooler REPL

**Kill the lopsided remainder margin — centre it instead of resizing the window.**
`cols`/`rows` are floor divisions of the usable pixel size, so an arbitrary window
leaves up to one cell of remainder. The first cut *snapped* the window
(`request_inner_size`) to a whole-cell multiple — but that's WM-dependent and a
compositor that ignores the request leaves the strip (brood-edit: "too much margin at
the bottom" on a non-maximised window). Replaced it with **`grid_origin`**: the grid's
top-left is `inset + remainder/2` per axis, so the sub-cell leftover is split evenly on
*every* edge (centred) rather than dumped at the bottom/right. WM-independent (no resize),
and `px_to_cell` shares the origin so clicks stay aligned. Dropped `snap_to_grid` and its
`pending_snap`.

**`gui-bg!` (new builtin).** Sets the window background — the `Op::Clear` / pre-clear /
inset-margin fill — so the padding around the grid matches the app's theme instead of the
hardcoded Catppuccin `DEFAULT_BG`. Global like `gui-inset!` (a `UserEvent::Background`,
a `Renderer.bg` field, `disabled` stub + `PRIMITIVE_DOCS`). brood-edit wires `(gui-bg!
*base*)`.

**Line-height** the magic `1.3` cell-height multiplier is now a named `LINE_HEIGHT = 1.4`
const — a touch looser so the grid reads as an editor, not a console.

**Slim scrollbar (brood-edit, pure Brood).** The pane scrollbar draws the `▕` right-eighth
block glyph per row (faint track + brighter thumb) instead of full-cell `rect` blocks — a
~1px modern rule via an ordinary `text` op + `:fg` face, no new kernel op (stays
cell-grid-only). `*inset*` 0→8 now that the margin is centred + themed.

**Cooler REPL (`std/tool/repl.blsp`, pure Brood).** Result output is re-lexed with the
input highlighter (`highlight-spans`) and ANSI-coloured on a TTY (plain on a pipe); a dim
`; N ms` note prints when an eval exceeds `*repl-slow-ms*`; Guile-style `,` meta-commands
(`,help` `,doc <name>` `,type <expr>` `,time <expr>` `,clear`) intercept at a fresh prompt
before the reader; a plain one-line banner (the ASCII wordmark was too much).

**Multi-line line editor (`std/editor/lineedit.blsp`).** The editor read one physical line
at a time (prior lines frozen in scrollback as read-only `:prefix`), so a `)` whose opener
was on an earlier line couldn't reverse-video its partner in place. Reworked it into a true
multi-line editor: `:text` now holds newlines, with `:complete?` (an Enter predicate) and a
`:cont-prompt` enabling multi-line mode. Enter inserts a newline while the form is
incomplete (the REPL's `repl--complete?` = `read-all` minus the E0002 incomplete signal),
else submits. C-a/C-e are line-aware; ↑/↓ move between the form's lines and fall through to
history at the top/bottom. Rendering moves up over the previous block (`:last-row`), clears,
reprints **every** line, and reparks — so a bracket pair lights up across lines in place
(verified in the op stream: opener on line 0 and closer on line 1 both `{:reverse true}`).
The single-line path is kept intact (horizontal scroll + signature hint, the common case);
multi-line kicks in only once the text has a newline. The REPL now does one multi-line
interactive read per form (the line-by-line accumulation stays for the piped path). The
old `:prefix` continuation + `match-echo` echo are superseded and removed. All pure pieces
unit-tested (geometry, Enter logic, in-place-match op stream); repl 14, lineedit 41,
observer 55 green; piped REPL multi-line + meta verified end-to-end. (The interactive
relative-cursor render can't be driven headless — needs a human at a TTY to eyeball.)

## 2026-06-28 — Minimize the builtin surface: crypto 21 prims → 2, tree-sitter grammars out of the default kernel

Two language-surface trims (ADR-006: write the language in the language; kernel =
mechanism, Brood = policy).

**Crypto.** `std/hash.blsp` was a thin pass-through over **21** near-identical Rust
prims (`%sha256`/`%sha1`/`%sha384`/`%sha512`/`%md5` × {string→hex, bytes→hex,
bytes→raw} = 15 digests, + 6 `%hmac-*`). The only genuinely-primitive part is
"run this algorithm over these bytes, get raw bytes" — the string-vs-bytes input
and hex-vs-raw output axes are pure formatting Brood can do. Collapsed to **two**
keyword-dispatched prims: `(%digest algo bytes)` and `(%hmac algo key msg)`
(`algo` ∈ `:md5 :sha1 :sha256 :sha384 :sha512`), both → a `bytes` value.
`std/hash.blsp` rebuilds every public name over them — `string->utf8-bytes` for
string input, a new pure-Brood `hash/bytes->hex` for hex output. The public
`hash/*` API is byte-for-byte unchanged (verified vs FIPS/known vectors). Net
**−19 Rust prims**, formatting moved into Brood. Updated the two direct
`%`-prim callers (`std/uuid.blsp` v3/v5 → `hash/md5-bytes`/`hash/sha1-bytes`;
`std/tool/package.blsp` tree hashing → `hash/sha256`/`hash/sha256-bytes`).
pbkdf2/chacha/random-bytes prims untouched.

**Tree-sitter (ADR-103 follow-up).** The kernel hardcoded `:ruby`/`:elixir` —
both grammar crates were in `default` features, so a stock build linked two
language parsers into the language core. Nothing real depended on them (only the
two grammar test files + docstrings; no ruby/elixir mode exists yet). Split the
feature: `treesit` is now the **generic mechanism only** (the tree-sitter runtime
+ positioned-CST projection, still in `default`); the grammars are opt-in
`treesit-ruby`/`treesit-elixir` (+ a `treesit-grammars` bundle). `language_for`'s
arms are each `#[cfg]`-gated, so a default build enumerates **no** language and
reports any `:lang` as "not built into this runtime (rebuild with
--features treesit-<lang>)". `make test` and `make install` opt into
`treesit-grammars`; the two grammar test files **self-skip** (runtime
`tree-sitter-parse` probe) so a bare `cargo test` stays green. Dynamic runtime
grammar loading (no compile-time enum at all) is noted as the end state. Full
suite 634/634 with grammars; the grammar suites skip cleanly without them.

**Conversion-pair collapse (same session).** Found a third redundancy: `string->bytes`
and `bytes->string` were duplicates of `string->utf8-bytes` / `utf8-bytes->string`
(byte-identical UTF-8 encode; the decode pair only differed in that the `utf8-`
one is more lenient — accepts a vector/list too). Removed the short pair, kept the
explicit `utf8-` pair (matches Brood's symmetric `X->Y`/`Y->X` convention and the
already-dominant `string->utf8-bytes`, 26 refs). Updated all call sites in
`std/{encoding,crypto,url,net/http}.blsp` + tests and the reader's escape-hint.
Net another **−2 prims** (builtin count 320 → ~300 across the session).

`log2`/`log10` were evaluated and **kept**: deriving them from `ln` in Brood is a
real correctness regression — `(/ (ln 1000) (ln 10))` = 2.9999999999999996 where
the libm `(log10 1000)` is exactly 3.0. The Game-of-Life `bitset-*` kernels stay
(perf kernels; explicitly declined).

**Downstream:** the sibling `hatch` web framework used `%sha1` (RFC 6455 WebSocket
accept-key) and `%sha256` (asset fingerprints / strong ETags) directly; migrated
to `hash/sha1` / `hash/sha256` (+ `(require 'hash)`) in `src/http/websocket.blsp`,
`src/web/{assets,static}.blsp` and the two affected test files. `nest test` in
hatch: 526/526.

**`Value::Bitset` removed entirely (same session).** The biggest single simplification: the `bitset` feature (13 `bitset-*` prims + a whole `Value::Bitset` kernel kind) had no consumer left — zero references in std/, tests/, or any in-repo `.blsp`; its only user was an external Game-of-Life GUI demo, now dead (confirmed with the user). Deleting it removed a `Value` variant and its arms across **8 kernel files**: `value.rs` (variant + `Tag::Bitset` + `BsId` handle, tag arrays 22→21), `heap.rs` (the LOCAL/old/prelude/RUNTIME `bitsets` slabs, `alloc_bitset`/`bitset` accessor, GC flush `flush_bitset`/`flush_rt_bitset`, poison/hash/equality/promote/verify arms — ~78 refs), `numeric.rs` (the impls + Game-of-Life fused kernels `bitset-life-step`/`-neighbour-sum`/`-planes`), `printer.rs`, `process/message.rs` (`Message::Bitset`), `dist/wire.rs`, `types/mod.rs` (the tag lattice). Kept the unrelated `bit-*` *integer* ops and the GUI cell-paint path (board is a bignum or byte string). Verified: clean build, full suite green (the lone failure is the user's concurrent WIP `lineedit--match-echo` test, unrelated), and the GC/concurrency suites pass under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`. Builtin count now ~287 (was 320 at session start). KI-4 (the bitset-as-`Str` GC bug) is now moot; noted as superseded in known-issues.md.
