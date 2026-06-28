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
