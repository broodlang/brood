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
- **2026-06-28** — Dependency refresh to latest stable (rustls/rcgen/lsp-server/rodio/glow + cranelift 0.133's `MemFlagsData`) + a docs-driven crate audit (smallvec `union`, `MemFlagsData::trusted()`, ropey borrow-not-clone, rcgen→aws_lc_rs dropping `ring`, dead `glutin-winit` gone); exact bignum-vs-float compare via `BigDecimal`
- **2026-06-28** — Exact Decimal/Float ordering (BigDecimal, no `to_f64` loss); tree-sitter incremental reparse (`Tree::edit` + prefix/suffix diff, keyed by buffer id); damage-only softbuffer GUI present (default; `BROOD_GUI_DAMAGE=0` opts out)
- **2026-06-28** — Tech-debt sweep: boxed `LispError` so `LispResult` went 144→24 B; clippy 582→0 warnings with a fatal `-D warnings` gate (design-intentional lints documented as crate allows)
- **2026-06-28** — Checker: unused `let`-binding lint (conservative scan, `_`-exempt, position-gated); goals reframe — gate what's provably local/static, advisory at reload boundaries
- **2026-06-28** — Checker: +25 `CURATED_SIGS` (equality, string conv/predicate/transform, `format`, search→int) for stdlib fns `infer_sig` can't walk
- **2026-06-28** — GUI less-TUI polish (`gui-bg!`, `LINE_HEIGHT`, centred sub-cell remainder — vertical later re-anchored to the top; slim scrollbar) + a cooler REPL (highlighted results, `,`-meta-commands) + a true multi-line line editor (`std/editor/lineedit.blsp`)
- **2026-06-28** — Minimize the builtin surface (ADR-006): crypto 21 prims → 2 keyword-dispatched (`%digest`/`%hmac`); tree-sitter grammars out of the default kernel (opt-in `treesit-<lang>`); removed `Value::Bitset` entirely (~287 builtins, was 320); `log2`/`log10` kept for libm exactness. KI-4 now moot.
- **2026-06-29** — GUI: `frect` sub-cell rounded-rect op (fractional cells, AA, alpha) → a macOS-style fade-in overlay scrollbar in brood-edit
- **2026-06-29** — Checker: fixed the `:use` / private-defn lint false positives — count unqualified + qualified refs; the private-defn check moved to a whole-project Brood pass (supersedes the 06-28 per-file Rust version)
- **2026-06-29** — Stability hunt: 4 correctness bugs fixed (float `=` JIT miscompile, `(apply f seq-view)` use-after-GC, `inf`/`nan` reader round-trip, `max`/`min` NaN/int-float divergence) + a 700-program × 4-engine differential fuzzer, 0 divergences
- **2026-06-29** — JIT use-after-free: an inlined arm's spliced `Chunk` dropped, dangling its baked `ConstVal` pointers on a throw-from-inlined-recursion — fixed with a process-lifetime `JIT_INLINE_CHUNK_KEEPALIVE`
- **2026-06-29** — Formatter: idempotence fix (recursive `had-author-newlines?`) + a pre-existing comment-drop bug; 208 `.blsp` files now idempotent + meaning-preserving
- **2026-06-29** — Checker: a user `(sig …)` is now authoritative for callers cross-module (persisted on the heap keyed by the qualified global symbol, via `%register-sig`)

---

## Recent — full entries

The last day or two in full; older sessions are condensed into the digest above,
their full text in [devlog-archive.md](archive/devlog-archive.md) (and git history).
Append new sessions below (newest last).

## 2026-06-30 — Checker: precise body inference (merely-wider returns) + int-closed arithmetic

The deferred type-system item — catching a function body that returns a value
*wider* than its declared return (not just one provably disjoint) — landed, after
removing the false-positive wall that had kept it deferred (ADR-011).

**The wall, and why it wasn't real.** A precise return-check naively warns on every
`int`-declared arithmetic function, because `+ - *` carry a blanket
`(number number -> number)` sig and `(* int int)` types as `number`. But the
checker *already* folds `BigInt → Tag::Int` (`value.rs:808`), so `int` means "any
integer" and `(* int int) -> int` is **sound** — an integer op on integers yields
an integer (i64 or bignum, both `Tag::Int`). No overflow/occurrence analysis
needed; the wall was just a coarse sig.

**Three pieces** (all in `crates/lisp/src/types/check/`):
1. `guards.rs` `numeric_call_ty`: int-closed rule — `+ - * quot rem mod abs` over
   all-`int` args → `int`; otherwise `None` (defer to the curated `number`, so
   float/mixed never narrow and no int-vs-float caller-check regression). `/`
   excluded. Wired into `expr_ty` before `sig_of`.
2. `guards.rs` `control_flow_ty`: `expr_ty` now types `if`/`do`/`when`/`unless`/
   `let`/`let*`/`letrec`/`cond`/`case`/`match`/`and`/`or` by unioning their result
   positions (threading let-RHS types into scope; narrowing `if` branches via
   `guard_assertion`). `None` if any contributing sub-form is unknown.
3. `walk.rs` `gradual_of_compound`: `gradual_of` recurses through the control-flow
   forms and joins branch `GradualTy`s. The load-bearing property — an all-precise
   body (literals, sig-params, int-closed arithmetic) stays **`stat`** → checked
   with `⊆` (catches a wider-than-declared body), while *any* over-approximated
   call branch makes the join **`dynamic`** → checked with `∩≠⊥` (defers, never
   over-warns). That precise/dynamic split is what keeps it false-positive-clean
   instead of flooding on every `number`-result call.

**Verified.** `(sig f (int -> int)) (defn f (x) (* x x))` no longer warns (the
int-int fix); `(sig f (int -> string)) (defn f (x) (* x 2))` warns "yields int";
`(sig f (int -> int)) (defn f (x) (if (> x 0) x "neg"))` warns "yields int |
string"; a body ending in an un-sig'd call defers; `(if (int? x) x 0)` narrows and
passes. Gates: **`nest check` zero new warnings** (the only diff vs baseline is the
17 `%blob-ptr` debug-builtin lines, a nest-build-flag artifact — confirmed 0
"declared return"/"yields" warnings on std/+tests/), `types::` 167, catalog 2/2,
full in-language suite green, clippy clean, 3 new regression tests.

## 2026-07-01 — CLI polish + repo hygiene: colored diagnostics, rustfmt gate, CI

A presentation/infrastructure pass (no language semantics touched). Four pieces:

1. **Colored diagnostics.** `cli_support::report_error` now renders rustc-style on
   a terminal: bold-red `error:` label and caret, bold message, bold-cyan `hint:`,
   dimmed version footer. Gated on `stderr.is_terminal() && NO_COLOR` unset
   (https://no-color.org) via a new `use_color()`, so a pipe / redirected stderr /
   the LSP / MCP consumers stay **byte-for-byte** plain and editor-parseable — the
   `FILE:LINE:COL:` prefix is never colored. Only the `<kind> error:` label is
   colorized within the located line (found by substring, always precedes the
   message). The ANSI is bare `&str` consts on the cold error path — no `crossterm`
   writer pulled in. The type-checker's advisory `warning:` line is still plain
   (a deliberately-scoped follow-up).
2. **`ErrorKind::label()`.** Centralized the `"error:"` / `"<kind> error:"` prefix
   in one method; `Display` now delegates to it (was a 6-arm match), so the label
   text has a single source of truth that `report_error`'s colorizer also reads.
3. **Clippy → clean on both feature sets.** The `set_capture_run` re-export was
   `#[cfg(test)]` but its only caller is a `#[cfg(feature = "jit")]` test, so a
   no-jit test build warned "unused import". Matched the cfg to the caller
   (`#[cfg(all(test, feature = "jit"))]`). `cargo clippy --workspace --all-targets`
   is now clean with `-D warnings` on **both** default and `--all-features`.
4. **rustfmt gate + one-time format.** Added `rustfmt.toml` (pins the defaults,
   `max_width = 100`) and ran `cargo fmt`. The tree was already ~99.5% conformant
   (p99 line width 95), so the diff is almost entirely wrapping the ~360 long-line
   outliers across 51 files, plus a couple alphabetized `use` groups — no semantic
   changes. `cargo fmt --check` is now a meaningful gate.
5. **CI.** First `.github/workflows/ci.yml` — a fast `fmt --check` job + a
   build/test job (`clippy --all-targets --all-features -- -D warnings`, nextest
   with `treesit-grammars`, doctests), mirroring `make check`. Installs the system
   libs the `--all-features` surface needs (ALSA, xkbcommon, X11/Wayland, GL).

**Verified.** `cargo build --workspace` green; clippy clean (default + all-features,
`-D warnings`); `cargo fmt --check` clean; colored output confirmed via a pty
(`script`), plain output confirmed byte-identical on a pipe. Pushed to `main`
(3 commits: feature, style, ci).

## 2026-07-01 — Vectors: inline small-vector storage (closes the `bintree` heap gap)

Closed the largest remaining compute gap from the benchmark suite (`bintree`,
was 6th/7). Root cause was the **vector representation**, not JIT coverage (both
hot arms already tier & lower): `vectors: Vec<Vec<Value>>` paid a **`malloc` per
vector** — `bintree` allocates ~1.6M 2-element `[a b]` nodes/run — and forced
`nth` reads through the `brood_rt_vector_ref` FFI (double indirection, the JIT
couldn't inline). Pairs by contrast use a flat `Vec<(Value,Value)>` bump slab
with JIT-inlined `first`/`rest`; this brings vectors to parity.

1. **Inline storage.** `vectors: Vec<Vec<Value>>` → `Vec<VecStore>`, where
   `VecStore` is a `#[repr(u8)] enum { Inline { len: u8, items: [Value; 2] },
   Spill(Vec<Value>) }` (`INLINE_VEC_CAP = 2` — the hot 2-tuple / seqview case;
   ranges & larger spill). It impls `Deref`/`DerefMut` to `[Value]`, so the
   macro-generated accessor and all ~50 `.vector()` readers are **unchanged** —
   only the alloc sites and a few direct-slab GC sites needed edits. `#[repr(u8)]`
   pins the layout for the JIT (tag @0, `len` @1, `items` @8), asserted by
   `vecstore_jit_layout`. Chose an enum over a struct-with-spill after a fat
   `[Value;3]+Option<Vec>` struct (104 B) regressed the GC-copy-bound `bintree`;
   the enum keeps a slot ≤ the old handle-plus-`malloc` footprint.
2. **Direct allocation** (the biggest lever). `brood_rt_make_vector2` did
   `alloc_vector(vec![a,b])` — a temp-`Vec` malloc+free *per node*. New
   `alloc_vector2(a,b)` bump-pushes an inline `VecStore` directly. This flipped
   `bintree` from a Phase-1 regression to a win.
3. **JIT-inlined `nth`.** New `inline_vec_ref` lowering (`jit_lower.rs`) for
   `(nth v <const>)`, the vector analog of the pair car/cdr inline: tag → region
   → age → (fetch `brood_rt_vec_nursery_base`/`_old_base` **per read**, so it's
   sound across GC safepoints — `check`'s non-tail calls) → spill-tag → bounds →
   `slot + items_off + i*24`, deopting to the VM on any slow case. Added
   `TAG_VECTOR = 10` (pinned in the value layout test).

**Verified.** All 643 tests pass; `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` clean on
the vector-heavy benchmarks; every benchmark checksum bit-identical to before.
`bintree` compute **−8.5%** (harness, 128→117 ms; N=2000 wall 0.97→0.83 s ~14%),
every other benchmark neutral (a harness `fib` +5.5% was thermal — re-measured
flat), memory neutral. Likely lifts `bintree` 6th→4th (past Python/Ruby) — pending
a full 7-language re-run. Follow-ups (low ROI, deferred): inline read at
variable-index sites; in-arm inline alloc (blocked by make/check safepoints).

## 2026-07-01 — JIT back-edge store-elision for carry loops — prototyped, REJECTED

Investigated the compute-frontier "float lowering" lever (`mandelbrot`). **Reverted,
NO-GO** — the premise was stale and the win was ~0.

**Finding: the premise was wrong.** `mandelbrot`'s `esc` floats are *already*
register-carried — the `carry_vars` path in `jit_lower.rs` carries f64 loop params
in Cranelift block-param phis, with native `fadd`/`fmul` and the reads seeded once
at entry (verified via `BROOD_JIT_DUMP_IR` CLIF: 6 float tag-checks = one per param
at entry, 9 native float ops). There is no boxed-read tax to remove. The only
residual per-iteration cost was the **back-edge stores** (~17 in `esc`) that box the
carried values into frame slots so a `deopt`/`preempt` can resume on the VM.

**What was built + how it was validated (all reverted).** A disciplined, staged
change: (1) split `deopt` → a non-materialising `entry_deopt` (hoist/seeding guards)
vs a materialising `deopt`/`preempt`; (2) materialise carry vars → slots lazily at
those exits; (3) elide the hot back-edge stores. Correctness was checked by a
temporary runtime poison flag (`BROOD_JIT_POISON_CARRY=1`) that wrote type-matched
garbage into the elided slots — any un-materialised resume-from-slots path then made
the VM read poison and the checksum diverge. Also added JIT-vs-`BROOD_NO_JIT=1`
differential + deopt-forcing programs (mid-body overflow deopt, spawned-process
preempt).

**Why NO-GO:**
1. **Zero gain.** Elided int loops measured flat-to-slightly-worse (best-of-9, high
   N): `loop` 0.26→0.28 s, `collatz` 0.27→0.28 s, `primes` flat. The back-edge stores
   are effectively free — the CPU store buffer / L1 absorb them; the store was never
   the bottleneck.
2. **Fragile.** The poison caught FOUR separate whole-`Value` slot reads that bypass
   the carry register and had to be made carry-aware for correctness: `store_op`
   (`exit_done` returns), `read_words`, the SelfCall passed-through-arg update, and
   `as_block_arg` (block-crossing). `Op::Slot(carry k)` leaks pervasively — every
   slot-read site would need carry-awareness, for no reward.

**Takeaway.** Carry loops (`mandelbrot`/`loop`/`collatz`/`reduce`) are near the JIT
floor; their residual gap vs .NET is the boxed 24-byte `Value` tagging *in the
arithmetic*, not the frame stores. Don't re-attempt store-elision. Higher-ceiling
lever is dispatch / env-lookup cost (`nqueens`/`pipeline`: ~325–398K env-hops;
`pipeline`'s transducer arithmetic runs as indirect closure calls). The
`BROOD_JIT_POISON_CARRY`-style validation (poison elided state → checksum diverges)
is a good technique to reuse for any future carry-slot change.

## 2026-07-01 — GC: scale the nursery threshold by *total* live (young+old); rarer majors

Profiling `sort` (5th/7) found its cost isn't the sort — the numeric single-arg
`(sort nums)` already uses the native `%sort-asc` — but **building the input list**.
Decomposing a 2M-element `cons` loop: ~9% arithmetic, ~24% per-`cons` overhead,
**~67% GC**. The collector was copying the growing, all-live accumulator far more
than necessary.

**Two root causes + fixes** (`core/heap.rs`):
1. **Nursery threshold used young-only live.** After each minor GC the threshold
   became `max(gc_floor, local_live_count*2)` — but a *tenuring* build moves its
   survivors to the old gen, so young ≈ 0 and the threshold collapsed to the floor
   (~64K), re-collecting every floor-worth of allocations → O(n/floor) minors while
   building one structure. Fixed to scale by **total** live `(young+old)*2`, so a
   large-live process earns a proportionally bigger nursery (O(log n) collects),
   while a small-live churny process (a `spawn` worker) still sits at the floor —
   no concurrency regression.
2. **Majors doubled (`old*2`).** During a large build the old gen is nearly
   all-live, so a major compacts the whole growing list and reclaims almost nothing.
   Grown to `old*MAJOR_GROWTH` (default 4, `BROOD_MAJOR_GROWTH` override) — those
   wasteful full-list compactions become geometrically rarer.

**Measured.** A 2M-list build: 33→5 collections, 4.26M→2.35M objects copied,
**0.44→0.32s (~27%)**; the `sort` benchmark **173→150 ms compute (~13%)**. All 643
tests pass; JIT-vs-VM differential clean; `BROOD_GC_STRESS`+`GC_VERIFY` clean. No
time regressions across the suite (`fib`/`loop`/`mandelbrot`/… flat), and lower peak
RSS on several rows. Memory-for-speed, but net memory is neutral-to-better because
the rarer majors cut the transient 2×-copy peak. General: helps any code that builds
a large sequence (`map`/`filter`/reduce-into/`cons` loops), not just `sort`.

**Follow-up (same day): cap the nursery threshold** (`NURSERY_MAX` = 8M objects). A
review caught that the total-live scaling was unbounded: `should_collect` fires a
minor when *young* ≥ threshold, so a process with a large live old gen that then
*churns* transient young garbage would buffer ~2×old before collecting — young memory
ballooning with old-gen size (a long-running large-heap process, e.g. the editor;
short benchmarks never hit it). Capped so the young buffer is bounded regardless of
old-gen size, well above real build working sets (`sort` needs ~750K, `gen` 2M needs
~4M — both under the cap, so the win is unaffected). Handle index is 32-bit, so the
larger nurseries can't overflow it. 643 tests + differential + GC-stress still green.

## 2026-07-02 — pfib parallel-scaling: kill the inline-upgrade epoch-bump cascade

**Root-caused + fixed the JIT parallel-scaling gap** (native code fanned out over many
green processes scaled ~1.9× where independent OS processes got ~4.4×). It was **not**
cache/TLB/scheduler — it was the two-stage-tiering inline-upgrade swap using the
**shared** `global_epoch` as its signalling channel.

**Mechanism.** Arms are per-process (`compiled_arm_for` caches each in the process's
own `vm_cache`; `share_key` shares only the small-native *code pointer*). When a
process's deferred *inlined* upgrade landed, the swap called `bump_global_epoch()` to
force its own call sites to re-validate and pick up the larger `inline_nslots` frame.
But `global_epoch` lives in the `Arc<RuntimeCode>` shared by every process, so the bump
made **every peer's** `arm.compile_epoch` stale → each peer hit the hot-reload guard in
`jit_tier`, nuked its installed `jit_code`, reset its inline flags, re-tiered,
re-enqueued its own inlined upgrade, re-swapped and **re-bumped**. With 100 processes
running `fib` concurrently the bumps cascaded endlessly, so nearly every call fell off
the in-IR fast-link onto the slow IC-dispatch path (`call_ic_hit`) — ~2–3× the
instructions.

**Fix** (`core/heap.rs`, `eval/compile/mod.rs`). The inline upgrade is local to one
process's own arm, so scope its invalidation to match: drop the `bump_global_epoch()`,
leave `compile_epoch` at the current epoch (the inlined operators were just
re-validated at compile time), and instead call a new per-process
`Heap::invalidate_fast_links_for(sym)` that clears just this process's `CallIcEntry::fast`
memos + their `FastLink` IR mirrors for the swapped callee. The next call re-probes
`vm_call_ic_fast_link`, picks up `inline_code` + `inline_nslots`, and stays linked.
Peers are untouched — no cascade. Removed the now-dead `bump_global_epoch`.

**Measured** (100×`fib`, this machine). The cascade only bites once tasks run long
enough for the inlined upgrade to land *mid-flight*: at `pfib` N=28 (the benchmark
default) the run finishes first, so old==new (~32B insns, 0.42s). At **N=32** the swap
lands early and the effect is stark: **337B→120B instructions (2.8×), 4.7s→1.6s wall
(2.7×)**. As a bonus the fix also stops the shared-JIT small-native cache from going
stale on every swap (peers no longer redundantly recompile). 643 tests pass;
JIT-vs-VM differential clean; `pfib` under debug-assertions + `BROOD_JIT_VERIFY` +
`BROOD_GC_VERIFY` and under `BROOD_GC_STRESS` both clean (the fast-link re-probe sizes
the callee frame correctly). Note: the published `pfib` benchmark uses N=28, so its
number is unchanged — the win is for longer-running parallel-native compute.

**Follow-up (same day): share the inlined native across processes.** With the cascade
gone, the next lever is that the *inlined* upgrade (worth ~1.7× on `fib`) was compiled
**per-process** — each of N spawned workers raced its own deferred compile, and for a
short fan-out most finished first, so the inline win never landed. The small native was
already shared across a runtime's processes (`RuntimeCode::jit_code_cache`, ADR-101);
the inlined native was explicitly not. That's exactly how the BEAM works (BeamAsm
compiles a module to native once at load; all processes share it) — Brood already
matched it for the small native, so sharing the inlined native completes the alignment.
Added `RuntimeCode::jit_inline_cache` (companion `(id, argc) → (ptr, epoch)` map): the
first process to land the inlined compile publishes it in the swap block (once per arm),
and a peer installs it, sizing its frame to its own `inline_nslots` — deterministic for
the shared bytecode, so the pointer is interchangeable. Safe because the swap is now
per-process/cheap (above) and the epoch guard flushes the cache on `def`/compaction just
like the small-native cache. Hardened the hot-reload guard to also null `inline_code` on
an epoch mismatch (a stale inlined pointer must not survive a redefine and get
re-published). New regression test (`jit_shared_spawn_test.blsp`): a *recursive*
(inline-eligible) fn shared+inlined across 500 workers returns the right checksum, and
redefining it invalidates the shared inline cache — the existing shared-JIT/hot-reload
tests use non-recursive fns that never inline, so this path was uncovered. Measured
(100×`fib`): N=30 **0.84s→0.56s** (1.79× vs small-native-only), N=32 **1.6s→1.42s**.
Still nothing at N=28 — at ~4ms/task the run is warmup/scheduling-bound, below the point
where any tier past the small native can land in time; the win is for substantial
parallel bursts (N≥30). 643 tests + differential + debug verifiers + GC-stress clean.

**Follow-up (same day): inline-depth measured — depth-2 is optimal (NEGATIVE result).**
Chasing the single-thread `fib` gap to Elixir (Brood ~32 ms vs ~11 ms per fib(31), ~3×),
tested whether inlining the recursion *deeper* helps. First a **refactor**: the self-inline
probe and `rederive_inlined_body` had duplicated two-pass logic with "must mirror exactly"
comments (a footgun — a divergence would size the inlined frame wrong → corruption); deduped
into one `build_inlined_body`, now the single source of truth for both. Added tunable
`BROOD_INLINE_DEPTH` (passes, default 2) + `BROOD_INLINE_MAXBODY` (per-pass expansion cap,
default 64) for A/B. Result (serial 100×fib(31), per-fib): depth-1 41 ms, **depth-2 34 ms
(default)**, depth-3 32.6 ms (within noise, and needs a global cap bump), depth-4 41 ms,
depth-5 38 ms. So depth-2 is the sweet spot — deeper saturates then **regresses** (the
ballooned arm thrashes i-cache in the tight recursion). Confirms the call-protocol overhead
is no longer the bottleneck; the residual ~3× to Elixir is the **boxed 24-byte `Value`
arithmetic** (box/unbox + tag-dispatch at every call boundary and every `+`/`-`/`<`), whose
fix is unboxed-`i64` register-carry through the recursive arm (the f64 carry already exists
for `mandelbrot`) — a real JIT project, not a surgical change. Default behaviour bit-identical
(depth=2, 6 blocks); 643 tests + differential clean.

## 2026-07-02 — unboxed-i64 register calling convention for int-only recursion (SHIPPED)

**Closed the single-thread `fib` gap to Elixir — and passed it.** The Increment-0 profile showed
~55% of `fib`'s time was the boxed recursive-call protocol (`jit_run_fast_link` + frame setup +
`brood_rt_push` staging), not arithmetic (which the JIT already unboxes into SSA registers). So an
int-only, single-arg, recursive arm (`fib`) now lowers to a **register calling convention**: a
compact worker `extern "C" fn(n: i64, depth: i64, ovf: *mut u8) -> i64` (in `jit_lower_i64_arm`,
`jit_lower.rs`) that recurses with args/results in registers — **no boxing, no roots-staging, no
fast-link dispatch, no GC/spills** (an i64 isn't a handle, so the worker needs no `heap` at all).
A thin boxed wrapper (`fn(heap, base) -> outcome`, the arm's actual entry) unboxes the arg from
`roots[base]`, calls the worker, and boxes the result back — or deopts (outcome 1 → VM) if the arg
isn't an `Int` or the worker overflowed. **Overflow correctness**: Add/Sub/Mul are overflow-checked;
on overflow (or a depth-cap bail — the native-stack guard, since this bypasses
`JIT_NATIVE_DEPTH_LIMIT`) the worker sets a per-process sentinel (`Heap::jit_i64_overflow`, via the
stable `brood_rt_i64_overflow_ptr`) and short-circuits the unwind (O(depth)); the wrapper deopts and
the VM recomputes with BigInt — so `fact(25)` still yields the exact bignum. i64-eligible arms **skip
the two-stage inline upgrade** (`arm_i64_eligible`, consulted in `jit_tier`) — the worker already
recurses to full depth in registers, so the boxed depth-2 upgrade would only swap in inferior code.

**Measured**: serial 100×`fib(31)` **3.28 → 0.77 s (4.26×)**; `fib` benchmark **227 → 53 ms, 5th →
2nd** (beats Elixir 75 ms & Node 79 ms); `pfib` N=31 **847 → 152 ms, 5th → 2nd (1.3× off .NET)**,
ahead of Elixir/Node/Clojure. Aggregate single-thread compute **3.5× → 3.0×, 4th → 3rd — now ahead
of Elixir.** Shipped default-on (`BROOD_NO_I64` opts out). Gates: 643 tests pass with the path forced
on; JIT-vs-VM differential clean; `fib`/`fact` correct; `pfib` under debug-assertions +
`BROOD_JIT_VERIFY` + `BROOD_GC_VERIFY` and under `BROOD_GC_STRESS` clean. First Increment of the
unboxed lever; next = broaden the subset (multi-arg, `let`, more ops — see todo.md).

**Follow-up (same day): Increment 2 — multi-arg + a deep-recursion cliff fix.** Generalized the i64
worker from single-arg to **N fixed args** (worker `fn(a0..a_{n-1}: i64, depth: i64, ovf) -> i64`;
`Local(k)` → param k; the wrapper tag-checks every arg). Decoupled eligibility from `inline_name`
(which the depth-2 inliner sets — and rejects e.g. Ackermann, whose inlined expansion is too big) →
now keys on `dbg_name` + no-capture + a has-self-call check. A 2-arg shallow-wide recursion (`fib2`)
gets **0.88 → 0.18 s (4.9×)**. **The important fix:** register recursion can't drain to the VM
mid-stack, so a deep *non-tail* recursion (`g(5000)`, depth 5000) hit the worker's depth cap and
deopt-and-re-tiered **per level — a ~127× thrash** (present in Increment 1 too, for deep single-arg
recursion — correctness was fine, perf catastrophic). Fixed with a distinct **depth-bail outcome
(5)**: the worker sets the sentinel to `2` (vs `1` for overflow), the wrapper returns 5, and
`jit_tier` permanently switches that fn to the **boxed path** (which drains deep recursion via
`jit_native_depth`/`jit_force_vm`) — marks it in a process-global `I64_TOO_DEEP` set (consulted by
`arm_i64_eligible` and the shared-JIT install so a stale shared i64 wrapper isn't re-installed),
drops `jit_code`, re-tiers. `g(5000)` and `f(30000)` now match the boxed path exactly (0.05 s /
0.21 s, was 6.3 s / 16.3 s); shallow wins unchanged (fib 0.77 s, pfib 0.17 s). 643 tests + differential
+ debug verifiers + GC-stress clean; `fib`/`fib2`/`ack`/`fact`/deep-recursion all correct. Increment 2
also added `let`/`do` bindings (binders in SSA vars, forward-refs rejected; `do` = last form, pure
subset — 4.7× on shallow-wide let recursion) and `rem`/`quot` (÷0 + `i64::MIN/-1` guarded → deopt to
the VM's exact error) + `bit-and`/`bit-or`/`bit-xor`.

## 2026-07-02 — remove `let*` (breaking): Brood's `let` is already sequential

**`let*` was a pure alias for `let`** — the macroexpand pass canonicalised `let*` → `let` (Brood's
`let` binds sequentially, so there was never a semantic difference). Having it around implied `let`
*might* be parallel (why else offer `let*`?), muddying the deliberate "`let` is sequential, full stop"
design, and it cut against the repo's own "keep the core minimal / no aliases" principle. Removed it:
dropped the `LET_STAR` keyword, the special-form-table entry, the macroexpand canonicalisation branch
(kept `lambda`→`fn`), the checker's `let*` recognition (walk/guards), and the `(special-forms)`
introspection entry. `(let* …)` now reads as a call to the unbound symbol `let*` (a clean "unbound
symbol: let*" — the teachable nudge to use `let`). Renamed `lambda_let_star_test.blsp` →
`lambda_test.blsp` (lambda-only); updated `docs/language.md` and the grammar tool's docstring examples.
643 tests pass; `let`/`lambda` unaffected. Greenfield break (no external users); `let*` had zero uses
in `std/`.

## 2026-07-02 — `spit-bytes`: the byte-faithful file write (write side of `slurp-bytes`)

Added a `(spit-bytes path bytes)` primitive (`builtins/io.rs`) — the binary write-side
counterpart to `slurp-bytes`, which had no inverse. `spit`/`spit-private` are UTF-8
string-only (they *reject* a `bytes` value), so there was **no way to write a byte
sequence to disk** — a real gap surfaced by the brood-chat file-sharing feature (a
received image's bytes cross the mesh fine — `send` deep-copies a `bytes` value
faithfully — but couldn't be materialised to a file). Low-level I/O the language can't
bootstrap, so a Rust builtin is the right layer (mechanism, not policy).

Mirrors the existing shapes exactly: `expect_string` for the path, `collect_bytes` (the
same coercion `%digest`/`%hmac` use) for the payload — so it accepts a `bytes` value, a
vector, or a list of byte ints 0–255, rejecting an out-of-range int rather than truncating
— then `std::fs::write`, `FILE_IO` error code on failure, returns nil. Registered in the
`def` table (`Sig::new(vec![string, any], nil_ty)`) + `PRIMITIVE_DOCS`. Tests folded into
`tests/slurp_bytes_test.blsp` (5 new): a bytes-value round-trip (asserting `spit` errors on
the same value), a direct non-UTF-8 write whose sha256 matches the OS digest (the binary
test there had used a `printf` subprocess *because* this primitive was missing — now
unnecessary), the three accepted input shapes, out-of-range rejection, and replace-not-
append. 9/9 pass. Purely additive — no existing code path touched.

## 2026-07-02 — `image-thumb`: decode + downscale an image to RGBA (inline previews)

Added `(image-thumb bytes max-w max-h)` (`builtins/io.rs`): decode an encoded image
(PNG/JPEG/GIF/WebP/BMP) from a byte sequence and downscale it to fit `max-w`×`max-h`
pixels (aspect preserved), returning `{:width :height :rgba}` — `:rgba` a `w*h*4`
bytes value (row-major RGBA8), or nil when the bytes aren't a decodable image or the
dims are non-positive. **Downscale-only**: a source already within the box keeps its
native size (`thumbnail`/`resize` would *upscale* a small image to fill the box).
Untrusted input degrades to nil rather than throwing; per-call `image::Limits`
(≤ 16384² px, ≤ 512 MB alloc) bound a decompression bomb. Map built like `file_stat`
(`map_from_pairs`; the `rgba` handle isn't held across an eval — a builtin never fires
GC mid-execution).

New dependency: `image = "0.25"` with `default-features = false` + only the common
web/raster codecs (`png jpeg gif webp bmp`), so no TIFF/EXR/… and their heavy
transitive deps. Justified under the "runtime crates that remove real complexity"
bar — a decoder + resampler is exactly the kind of thing the language can't
reasonably bootstrap, and it's *mechanism*: rendering stays Brood **policy** over the
decoded buffer (brood-chat renders received images inline as upper-half-block `▀`
cells — 2 px/cell via truecolor fg/bg — which works identically in the terminal and
GUI with zero `gui.rs` changes). Registered in the `def` table
(`Sig::new(vec![any, int, int], map_ty.union(nil_ty))`) + `PRIMITIVE_DOCS`. New
`tests/image_test.blsp` (3): decode a hand-built 2×2 PNG fixture (inlined as a byte
vector — no disk) to the right dims + first pixel, downscale-to-fit, and
nil-on-non-image / non-positive-dims. 3/3 pass.

## 2026-07-02 — unboxed register worker: f64 sibling (float recursion)

Completed the unboxed lever with a **float** worker, by **parameterizing** the whole i64 worker on a
`Scalar {Int, Float}` kind rather than duplicating it (avoids the "two copies must mirror" footgun):
`const`/`arith`/`cmp`/`box` switch on the kind; the depth-bail cliff, `let`/multi-arg, poisoned
unwind, and the boxed wrapper are shared. **Float is simpler than int**: no overflow (IEEE `inf`/`NaN`
are valid results → it never deopts for arithmetic), ordered `fcmp` (NaN→false, matching the VM's Rust
`<`). The float arith subset is `+ - * /` only — `min`/`max` are excluded because Cranelift
`fmin`/`fmax` NaN semantics differ from the VM (those arms fall to boxed). An arm's kind is pinned by
its base-case threshold const (`(< x 2)` → Int, `(< x 2.0)` → Float); a mixed-int/float body matches
neither and stays boxed (and the wrapper tag-checks every arg, so a wrong-typed call deopts). Unboxed
float recursion is a large win (boxed float fib is *very* slow — box/unbox per f64).

Validated hard (the "go chaotic, no crashes" pass): 643 tests on the parameterized code (i64 not
regressed); a differential fuzzer over ~1600 chaotic terminating programs (int + float; random
arities/shapes/ops, adversarial consts incl. `i64::MAX/MIN`, `0`, `1e100`) — **0 crashes, 0 JIT-vs-VM
mismatches**; boundary torture (exact cliff depths, `i64::MIN/-1`, overflow-at-depth, `inf`); a
400-process concurrency + hot-reload chaos; and `BROOD_GC_STRESS` — all clean, under the use-after-GC
tripwire + `JIT_VERIFY` + `GC_VERIFY`. Remaining deferred: int `Div` (inexact→float→deopt) and the
scoped unboxed-array lever for `matmul`.

## 2026-07-02 — HOF closure-call fast path in `range_reduce` (modest, and a redirect)

Profiled the next frontier (`nqueens` ~14×, `pipeline` ~7×) before building — and it **redirected off
allocation**: both are call/dispatch-bound, not alloc-bound (`nqueens` `cons` ~0% in the profile;
`pipeline`'s `eduction` is fused). The cost is per-element closure dispatch: a `reduce` over N
elements calls the *same* step closure N times via `apply_value → dispatch`, which re-resolves the
arm (`vm_cache_arm`) + re-runs passthrough/arity matching each element. A ceiling spike showed a
user-closure fold is ~60× a primitive one.

Fix (default-on, `BROOD_NO_HOF` opts out): `range_reduce_slow` resolves the step closure's arm ONCE
(`hof_resolve`) and calls it per element via a cached-arm `vm_apply` (`hof_apply_step`) — re-reading
only the rooted closure for its captured env (GC-safe) and re-checking its id (late-rebind falls
back). Measured **nqueens 0.53→0.48 s (~9%)** and a light-closure `range-fold` **1.87→1.52 s (~19%)**;
643 tests pass with it forced on, reduce differentials clean.

**Honest scope:** this only removes dispatch's *self*-overhead, not the per-call `push_frame`/
`vm_run_bc` protocol (the bulk — so not the 60× ceiling, which was a *passthrough* artifact), and it
doesn't touch `pipeline` (whose `eduction` folds through the JIT'd Brood `reduce`'s in-IR call path,
not the Rust `range_reduce` driver). The real win is the **lean-native-call lever** (call a JIT'd
closure directly via the fast-frame protocol, skipping the per-element trampoline) — scoped in
`todo.md`.

## 2026-07-02 — `todo.md` triage: int `Div` shipped for the unboxed-i64 worker; two items were already done

Went through `todo.md`'s open items looking for small, well-scoped work. Two turned out to already
be shipped, just never checked off:
- **Operand-position unbound lint** (noted as "attempted TWICE, reverted, real checker work needed"):
  `Ctx::enable_operand_checks()` is unconditionally on in `check_file`, which now macroexpands +
  pre-loads the whole project image before walking — so both blockers cited (unbound `match`-pattern
  vars, cross-file globals) no longer apply. `nest check` across the whole repo produces zero
  `unbound symbol` false positives (checked `pattern_matching_test.blsp` specifically — the file the
  original false positive was on), and correctly flags a real unbound name on a scratch file.
- **GoL finding #4** (spawned-process GC threshold vs. the depth-1 path ballooning to ~1.1 GB): already
  fixed — `gc_floor()` (`core/heap.rs:319`) divides the pre-GC object budget across the live process
  count, and its own doc comment names this exact scenario ("the fix for the `pfib` 1-GB blowup").

The one real gap was **int `Div` in the unboxed-i64 register worker** (`jit_lower.rs`). Unlike
`Rem`/`Quot`, `Scalar::Int`'s arith set never included `Div` — so *any* int-recursive arm using `/`
was ineligible for the worker at all (`arm_scalar_kind` bailed the whole arm to the boxed path, not
just that one operator). Fixed by adding `Div` to `i64_arith_op`'s `Scalar::Int` case and extending
`lower_i64_arith`'s `Rem | Quot` guard block to also cover `Div`: the existing ÷0 and `i64::MIN/-1`
guards apply unchanged, plus one more — a nonzero `srem` remainder ("inexact") — since the VM's `/`
on two ints only returns an `Int` when it divides evenly (`compile/mod.rs` `prim_apply`'s inline fast
path); anything else is a deopt to the VM, which builds the `Float` the worker can't represent.
Mirrors the already-proven `Rem`/`Quot` guard shape exactly.

Added coverage to `tests/unbox_torture_test.blsp`: exact `/` stays in the worker end-to-end
(`ut-div`), an inexact `/` deopts to the same `Float` the boxed path (`BROOD_NO_I64=1`) produces
(`ut-div-inexact`), and ÷0 raises the VM's exact error (`ut-div-slash0`). Verified clean under
`BROOD_JIT_VERIFY=1`+`BROOD_GC_VERIFY=1` (debug-assertions build), `BROOD_GC_STRESS=1`, and the full
643-test suite (`make test`, incl. the in-language `brood::suite`). Clippy clean.

## 2026-07-03 — HOF native fast-frame: `hof_apply_step` jumps the step arm's native code (nqueens ~18%)

Picked up the "close the `nqueens`/`pipeline` gap" frontier. **Profiled before building** — and both
profiles redirected off the two planned levers:
- **Front (b)** (make capturing closures fast-linkable by dropping the `!capture_names.is_empty()` bail
  in `vm_call_ic_fast_link`) targets the **elided free-global** in-IR fast-link. `perf` shows that path
  is **0%** of `pipeline`: its transducer steps are computed-head (captured `rf`/`f`), which never reach
  the elided fast-link. Dropping the bail would move `pipeline` ~nothing.
- **Pure computed-head arm-caching** is only ~1–6%: the `vm_cache` map is already FxHash-keyed (one
  multiply — `SymbolHasher`), so `vm_cache_arm` is mostly the `arm_for` scan + `Arc` clone, and
  `push_frame` does the same slot+capture work a fast-frame would.

What both profiles actually point at: the per-element step call runs through
`hof_apply_step` → `vm_apply` → **`vm_run_bc` trampoline + per-call `jit_tier` re-entry**, *even when
the step arm has installed native code*. On `nqueens` N=12: `push_frame` 13% + `vm_run_bc` 13% +
`jit_tier` 11% + `vm_apply` 4% — a quarter-plus of runtime is the VM call protocol wrapping a step body
that's already JIT'd.

**Fix (default-on, `BROOD_NO_HOF_JIT` opts out): `hof_apply_native`.** When the resolved step arm has
installed, epoch-current native code, `hof_apply_step` stages the args + captures and **jumps the
native entry directly** — skipping `vm_apply`/`vm_run_bc` (frame save/restore, loop safepoints) and the
`jit_tier` re-entry. It mirrors the proven computed-head native-link block in `jit_dispatch_call`: same
frame setup, the same `capture_value` fill (the fast frame bypasses `push_frame`, so captured lexicals
— `nqueens`' `placed` — are filled here; `capture_base == argc` since `hof_resolve` proved fixed-arity,
no optionals/rest), the same env-rooting across the call (re-read the live id after `f()` for the deopt
path), and the same 0/3/4/deopt outcome handling (outcome 4 follows the staged tail chain; a deopt
re-runs on the VM with the GC-updated args). `hof_apply_native` in `eval/compile/mod.rs`.

**Measured (best-of-3, clean `--release --features jit`):** `nqueens` N=12 **2.45→2.01 s (~18%)** with a
clean A/B (`BROOD_NO_HOF_JIT=1` = 2.45, default = 2.01), checksum `14200` identical both ways. `fib`
(no HOF — control) flat; `sort`/`matmul`/`wordcount` flat (no user-closure step). `pipeline` **flat** —
its dominant path is the VM `dispatch` computed-head branch, not `hof_apply_step`; closing it is a
separate (riskier) core-dispatch lever, deferred.

**Gate.** All 643 tests pass (`make test`, default). `nqueens` + `pipeline` clean under
`BROOD_JIT_VERIFY=1`+`BROOD_GC_VERIFY=1`+`BROOD_GC_STRESS=1` (debug-assertions build) — no stale-handle
or tripwire reports. Added a regression test to `tests/sequence_test.blsp`: a `reduce` with a
**capturing** step (`bias`) over a 5000-range (well past the tier threshold of 8, so the native path is
taken) must equal the closed form — a capture-fill bug would read `bias` as nil. Passes with the native
path on and off.

## 2026-07-03 — `%isolate` made RUNTIME-compaction-safe (silent global-misdispatch bug)

Found while investigating a forwarded report of `nest test` OOMing in the brood-edit suite (a
separate, still-open leak — see below). `%isolate` (`system.rs`) snapshots the global table — an
off-graph `SymbolMap<Value>` holding raw RUNTIME handles — runs a thunk, then `restore_globals`. If a
**RUNTIME compaction fired *during* the thunk** (its `def`s crossing `BROOD_RT_GC_FLOOR`, trivially
true in a large image), it relocated those handles; the off-graph snapshot wasn't rewritten, so the
restore reinstalled **stale handles that now aliased other closures** — an unrelated pre-isolate
global silently misdispatched (e.g. a 0-arg `foo` resolved to a 1-arg `z-*` defined *inside* the
rolled-back isolate → arity error, or worse a silent wrong value). No GC tripwire fires: the handles
are valid RUNTIME indices, just semantically wrong.

**Confirmed** by an A/B on the compaction floor (`scratchpad/iso_min.blsp`): `BROOD_RT_GC_FLOOR=1e9`
(compaction off) → correct; `=128` → misdispatched at ~300 defs. Latent for every `:isolated` test
(the runner wraps each in `%isolate`) once an image holds >4096 runtime closures.

**Fix:** a re-entrant `Heap::rt_collect_block` counter; `rt_gc_due()` returns false while >0, and
`%isolate` brackets its snapshot→restore window with `begin/end_rt_collect_block`. Deferring
compaction to *after* the restore is also strictly better — the isolate's own `def`s are garbage by
then, so the next safepoint reclaims them (verified: 10k-def isolate → rt-closures 3→10003 during →
back to 4 after the block lifts + a collect). Sound against concurrency too: `runtime_collect_with`
only compacts when a heap *uniquely owns* the runtime `Arc`, which during an isolate is either this
(now-blocked) process or impossible (other live processes ⇒ not unique). Scope: covers the
auto-compaction path (the real trigger); an explicit `(runtime-collect)` *inside* an `%isolate` is out
of scope (and semantically dubious).

Regression test: `crates/lisp/tests/runtime_collector.rs`
`isolate_is_safe_against_a_runtime_compaction_inside_the_thunk` — a low RT floor + a 500-def isolate;
a pre-isolate `(probe)` must still return 42. Gate: full suite (default + debug-assertions), clippy
clean.

**Still open (not this commit): the test-runner memory leak.** Loading N diverse files into one
long-lived image `def`s every top-level form into the shared `RuntimeCode` region + global table;
those are globally rooted → live, unbounded, JIT-independent, and `runtime-collect` can't reclaim them
(only same-name redefinition frees the old version). Probe: 20k distinct defs → ~130 MB,
`:runtime-closures` 3→20003, unchanged by collect (~6.4 KB/distinct def). `%isolate` does NOT bound it
(code slabs are append-only regardless of binding rollback). The 1 GB OOM in a 725-test suite is this
accumulation. Fix is architectural (run each file/batch in a reclaimable scope or a fresh child
runtime) — tracked next.

## 2026-07-03 — Test-runner memory leak fixed: run each file in its own rolled-back scope

The shared-RUNTIME accumulation flagged in the previous entry. Root cause (confirmed): `nest test`
(`run-project-tests`, `project.blsp`) loaded **every** test file into one long-lived driver image
(`(fold (fn (_ f) (load f)) nil files)`) before running any — so every file's top-level `def`s
promoted their compiled closures/chunks into the shared `RuntimeCode` region + global table, all
globally rooted → live, unbounded, JIT-independent, unreclaimable (only same-name redefinition frees
the old version). A 725-test suite crossed the 1 GB soft cap → `memory limit exceeded` on whichever
workers were allocating (the brood-edit suite: 9 spurious "failures", all passing in isolation). Probe:
20k distinct defs → `:runtime-closures` 3→20003, unchanged by `runtime-collect` (~6.4 KB/def).

**Fix:** `test/run-tests-scoped` (new, in `test.blsp`) runs the suite **file-by-file, each file inside
its own `%isolate`** (reset-units! → load the one file → drain its units → rollback). The rollback
drops that file's `def`s, and — because `%isolate` is now RUNTIME-compaction-safe (the
`rt_collect_block` fix earlier today) — the next safepoint reclaims the promoted code. `run-project-tests`
builds one loader thunk per file and calls it; `BROOD_TEST_NO_SCOPE` reverts to the legacy
load-all-then-run path (escape hatch for a suite relying on cross-file top-level defs). Tallying was
already process-local (returns/messages, not shared counters — "SHARE-SAFE TALLYING"), so aggregating
across per-file scopes needs no shared state. Files run sequentially (each `%isolate` blocks to
completion); tests **within** a file still parallelise — a modest cost (brood-edit: 24 files).

**Measured / gated:**
- **brood-edit (the reported OOM): 725/725 pass at 199 MB peak** (was >1 GB / OOM with 9 failures).
- Probe: 300 files × 200 distinct defs → unscoped `:runtime-closures` 60003 (the leak) vs per-`%isolate`
  scoped **3** — bounded. Rust regression test `per_isolate_scoping_bounds_runtime_region_growth`.
- No regression on other projects: pong 103/103, store 19/19. **hatch improved** — 526/526 at a 4 GB
  cap under scoping, where the legacy load-all path *fails* even at 4 GB (its large-payload tests are
  data-heavy; scoping removes the code accumulation on top). brood-life's 5 failures are pre-existing
  and unrelated (its `bitset` builtin is unbound in the current runtime — fails standalone).
- brood repo `make test`: 644 pass (the runner change is baked into the lib the cargo tests use).

## 2026-07-03 — Audit: two more "off-graph RUNTIME handle across compaction" bugs (declared_sigs, positions)

After fixing the `%isolate` compaction-unsafety (bug #2 above), swept for the same class:
structures that reference RUNTIME data (by handle or slab index) and persist across a RUNTIME
compaction but aren't in the set `runtime_collect_with` rewrites. Enumerated every `RuntimeCode`
field; two gaps, both from a pre-ADR-091 "a RUNTIME pair never moves" premise that compaction
invalidated:

- **`declared_sigs` (real — corrupts the checker).** The `(sig …)` table holds promoted RUNTIME
  type-expression `Value`s off the graph. A compaction relocated them out from under the stored
  handles, so `sig_of` later read a garbage form. **Confirmed** with a test: `(int -> int)` read back
  as `(i 1)` after churn+compact. Fix: `runtime_collect_with` now `flush_rt_value`s `declared_sigs`
  alongside `globals`. Regression test `declared_sigs_survive_a_runtime_compaction`.
- **`positions` (minor — diagnostics only).** The RUNTIME form-position table is keyed by pair slab
  *index*; a relocation strands its keys on recycled pairs, so `(form-pos …)` / source-location would
  return a stranger's position (or none) after a compaction. Fix: remap the keys through the same
  `fwd.pairs` forwarding (dropping entries whose pair didn't survive), right after the evacuation
  walks populate it.

Safe (audited, no fix needed): `globals` (already rewritten), `def_sites` (Symbol→SourceLoc, no
handles), `jit_code_cache`/`jit_inline_cache` (version-guarded — the compaction's `version` bump
flushes them), and the per-process caches (`vm_cache`/`global_ic`/site ICs, dropped in step 4).
Gate: 646 tests (default + debug-assertions), clippy clean.

## 2026-07-03 — Complete the test-runner leak fix: the `nest mcp` structured path too

Follow-up sweep on the bug-#1 fix found it incomplete: only `run-project-tests` (nest test) was
rewired to the per-file scoped run; `run-project-tests-structured` — which backs the `nest mcp`
run-tests tool — still loaded every file up front. That's the *worst* place for it: `nest mcp` is a
long-lived hot-reload image, so each run-tests call would re-accumulate every file's promoted code.

Factored `drain-files-scoped` (the per-file `%isolate` reset→load→drain loop) out of `run-tests-scoped`
and added `run-tests-scoped-structured` (same scoping, returns the `{:total :passed :failed
:failed-assertions :ms :results}` map instead of printing/raising — the memory-bounded twin of
`run-tests-structured`). `run-project-tests-structured` now uses it (with the `BROOD_TEST_NO_SCOPE`
escape hatch, matching `run-project-tests`). brood-edit `nest test` still 725/725 @ 199 MB after the
refactor; brood repo `make test` 646 pass.

## 2026-07-03 — Harden the KI-6 fix: compaction-safety moves into snapshot/restore itself

The KI-6 fix bracketed the compaction-block in `%isolate` (the one caller). That's fragile — a future
caller of `snapshot_globals`/`restore_globals` would silently reintroduce the misdispatch. Moved the
invariant to where it belongs: `snapshot_globals` now increments `Heap::rt_collect_block` (a `Cell<u32>`
so the `&self` methods can bump it) and `restore_globals` decrements it, so **"no RUNTIME compaction
while a globals snapshot is outstanding" holds structurally** — every caller of the protocol is
covered, `%isolate`'s explicit begin/end calls are gone.

Also closed the KI-6 caveat: the block is now checked at the `runtime_collect_with` **choke point**
(the single path both the auto safepoint — via `rt_gc_due` — and a manual `(runtime-collect)` funnel
through), so an explicit collect *inside* an `%isolate` is a no-op instead of a snapshot-stranding
corruption. New regression test `manual_runtime_collect_inside_isolate_is_a_noop`. Gate: 647 tests
(default + debug-assertions), clippy clean, brood-edit 725/725 @ 199 MB.

## 2026-07-03 — Harden snapshot/restore against unpaired calls (KI-6 follow-up)

The KI-6 fix made compaction-safety structural via `snapshot_globals`/`restore_globals`, but the
protocol was still misusable: a snapshot without a restore leaves compaction suppressed forever (the
leak returns), and a restore without a snapshot / a double-restore under-releases the suppression
(re-exposing an outer snapshot to KI-6). Closed all three at the type level:

- `snapshot_globals` now returns a `GlobalsSnapshot` newtype (private fields, constructible only
  here) — the sole type `restore_globals` accepts. So a restore can't run without a paired snapshot
  (nothing else can forge one), and it's taken **by value** so the same snapshot can't be restored
  twice (move error).
- `#[must_use]` on `GlobalsSnapshot`: an ignored/forgotten snapshot is a compiler warning.
- A `block_depth` token + a debug-only LIFO assert in `restore_globals` catches out-of-order restores
  (which would release the wrong scope's suppression).

Regression test `nested_globals_snapshots_suppress_then_re_enable_compaction` (nested snapshots each
suppress compaction, LIFO restore, then re-enable). Gate: 648 tests (default + debug-assertions),
clippy clean, brood-edit 725/725 @ 199 MB.

## 2026-07-03 — Scale-test the test runner (100K+ files): two more O(n²) fixes

Stress-tested the scoped test runner by generating a project with thousands of minimal test files.
It surfaced quadratic scaling (1K→4.5s, 2K→17.7s, 4K→72s) hiding under the 24-file brood-edit suite.
Two independent O(n²) causes, both fixed:

- **`drain-files-scoped` result aggregation** used `(fold (fn (acc r) (append acc r)) …)` — `append`
  recopies the growing accumulator every file → O(files²). Fixed: `cons` each file's result list
  (O(1)) then flatten once → O(total).
- **`drain-runner` leaked a monitor `:down` per call.** It `(monitor d)`'d the driver but never
  demonitored; the driver exits right after sending `:all-results`, firing a `:down` that matches no
  future `receive` (pinned to an old `mref`/`d`) → it piled up in the one long-lived scoped-runner
  process's mailbox, and every later `receive` scanned past all of them → O(files²). Fixed with the
  gen-call idiom (`demonitor` + flush the late `:down`, std/proc/gen.blsp). This is a general bug —
  any code that repeatedly `monitor`s a short-lived worker and abandons the monitor leaks `:down`s.

**Still open (a deeper interaction):** with both fixed, a residual quadratic remains, and it's
**JIT-driven** — `BROOD_NO_JIT=1` runs flat (2K & 4K both ~39s) where JIT-on is quadratic (11.5s→42s).
Root cause: the per-file `%isolate`'s `restore_globals` bumps the global `version` every file →
invalidates the epoch-guarded JIT code cache → the framework arms (`run-driver`/`drain`/…) re-tier and
recompile into the never-freed GLOBAL_JIT module every file. So a huge suite re-JITs the runtime per
file. Fixing it needs the version bump to not invalidate arms whose inlined globals are unchanged (the
baseline is identical after each rollback) — deferred; scoping the JIT-cache epoch more finely, or not
bumping version on a no-op restore. Until then a ~100K+-file single suite is impractical (the runtime,
not the tests, dominates); real suites (tens–hundreds of files) are unaffected and bounded in memory.

## 2026-07-03 — Root-caused the scoped-runner quadratic: it was two O(N²) bugs (both fixed)

The earlier "append O(n²)" fix was incomplete (a no-op, in hindsight). Root-caused the real quadratic
with clean per-count project dirs + per-file instrumentation (lesson: NEVER trim/regen a test dir in
place — it silently made every cross-count comparison use the same file count and produced a bogus
"JIT-driven" conclusion). Two independent O(N²) causes in `nest test`'s scoped runner:

1. **The result flatten used the VARIADIC `append`, which copies BOTH args.** `drain-files-scoped`
   folded `(append file-results acc)` — and `append` (`reverse (fold append--onto nil lists)`) recopies
   the growing `acc` every file → O(files²). (The earlier commit only reordered the args, still calling
   variadic `append`, so it never actually fixed this.) Fix: `append-two`, which copies only its first
   arg and SHARES the accumulator tail → O(total).
2. **`check-project` bloated the per-file snapshot baseline.** The advisory pre-flight loads every
   source + test file into the image for cross-file checking, leaving all N test modules bound in the
   global table. The scoped run then `snapshot-globals`-clones that O(N) table once PER FILE → O(N²).
   Fix: run `check-project` inside `%isolate` so its bulk loads roll back, keeping the run's baseline
   small (prelude + src + test framework). Warnings are stderr I/O, unaffected by the rollback.

**Measured (clean per-count dirs):** 4000-file suite 41s → **2.4s** (17×); now linear at ~0.6 ms/file
(1K→0.57s, 2K→1.3s, 4K→2.4s, 8K→4.9s), memory bounded. A 100K-file suite is now ~60s and a 1M-file
suite ~10min — the runtime no longer dominates. Gate: 648 tests + brood-edit 725/725.

## 2026-07-03 — check-project O(n²): root-caused, NOT fixed (investigation record)

Stress-testing `nest test`/`nest check` at 1K–100K files (after fixing the test-runner scoping, see
above) exposed a quadratic in the advisory whole-project checker: `nest check` 1K→1.8s, 2K→6.5s,
8K→87.6s; 100K `nest test` = ~78 min (check-project dominates; test execution itself is now linear).
Root cause is DIFFUSE — `check_file` runs once per project file and:
1. rebuilds `known_ns` by scanning ALL global symbols (`check.rs`), and
2. `%refer` (from each file's `(:use …)`) scans ALL globals (`system.rs`), and — DOMINANT (~80%) —
3. re-macroexpands + re-evals + re-**compiles** each file's `(defmodule … (:use …))` header, which
   grows with the loaded image (N distinct headers compiled into an ever-growing JIT module).

**Attempted + reverted:** caching #1/#2 on the heap. Two keys, both structurally wrong:
- **count-key** is UNSOUND — `%isolate` rollback removes globals, so a rollback-then-def collides on
  the same count with a different name-set → stale cache → broke 3 `(:use)` namespace suite tests
  (the exact hot-reload hazard flagged during review).
- **epoch/version-key** CHURNS — `global_epoch` is bumped by the JIT's inline-upgrade swaps (which
  fire during the check), so the cache rebuilds per file → no win (26s→21s, still O(n²)).

Even a perfect scan-cache only removes ~20% (measured); the dominant ~80% is the per-file header
re-processing, untouched. Reverted clean. **Real fix (deferred):** a `check_file` change that resolves
each file's imports WITHOUT re-eval/re-compiling already-loaded headers (in a whole-project check the
image is fully loaded up front) — a checker/loader/import-resolution redesign, higher blast radius,
best done fresh. Real projects (tens–hundreds of files) check fine; this only bites pathological
thousands-of-files suites. See `todo.md`.

## 2026-07-03 — Record/shape types: `(record :k T …)` slice 1 (ADR-115)

Added a heterogeneous, keyword-keyed map-shape type to the `(sig …)` grammar —
distinct from the already-shipped uniform `(map K V)` (`docs/type-map-kv.md`).
`(record :name string :age (optional int))`: fields required by default, `(optional
T)` marks one as allowed to be absent/`nil`; records are open (extra keys allowed).
Slice 1 only, staged exactly like `(map K V)`'s own slices: `type-matches?`
(`std/prelude.blsp`) enforces it at the `sig!`/`BROOD_CONTRACTS=1` runtime-contract
boundary — required fields need no separate presence check, since `(get v k)` on a
missing key is `nil` and `type-matches?` on the bare field type fails on its own
unless that type accepts `nil`. `parse_type` (`annot.rs`) accepts the annotation for
the static checker as flat `Ty::of(Tag::Map)`, validating every field's type so a
malformed record (odd field count, non-keyword key) is dropped rather than guessed.
No `Ty` struct change — a full `fields` refinement (real width/depth record
subtyping, field-wise union/intersect, literal-keyword `get`/`assoc` sinks,
record-literal type inference — `expr_ty` has no `Value::Map` arm at all today) is
real algorithm design, not copy-paste from `map_kv`, and stays deferred (ADR-011)
until a concrete consumer needs it — see `docs/type-records.md`. New `describe`
block in `tests/contract_test.blsp` + a Rust parse/no-panic test in `check.rs`; 46/46
`contract_test.blsp` and the full `types::check` Rust suite (168 tests) green.

## 2026-07-03 — Record/shape types: full `fields` refinement (ADR-115, slice 2)

Continued past the initial grammar+runtime slice in the same session: `Ty` gained a
`fields: Option<Arc<BTreeMap<Symbol, (Ty, bool)>>>` refinement (name → type,
required?), tagged `MAP_BIT` like `map_kv` — no new `Tag`. Three new pieces:
**width/depth record subtyping** (`record_fields_is_subtype` in `types/mod.rs`,
deliberately conservative — a field `self` doesn't declare, even one `other` marks
optional, makes subtyping return `false` rather than reason about absence; sound,
not complete, per contract #5); a **`get`-by-literal-key sink** (`check/guards.rs`)
resolving `(get r :name)` to the exact field type when the key is a literal
keyword; and **record-literal type inference** — `expr_ty` previously had no
`Value::Map` arm at all (vectors already infer `vector_of(element_union(…))`, maps
inferred nothing), so `{:a 1}` now infers a record shape with `:a` required,
type int, from the literal itself, no `sig` needed. Union/intersect deliberately
reuse the existing generic `merge_union`/`merge_intersect` helpers rather than a
fancier field-wise algorithm — the blunt widen-unless-identical rule is already
sound for every other refinement, so records get it for free. `is_disjoint` stays
tags-only, untouched.

Verified soundness two ways: targeted unit tests for subtyping/union/disjointness
(`types/mod.rs`) and the `get`/literal-inference sinks (`types/check.rs`); and, since
the literal inference touches the type of every `{…}` map literal project-wide, a
direct diff of `nest check` output across all of `std/` + `tests/` with the new
`expr_ty` arm disabled vs. enabled — byte-identical, zero new warnings. Full
`make test` green (649/649) before this slice; `cargo test -p brood --lib types::`
green after (173/173, up from 168 at slice 1). See `docs/type-records.md` for the
full design and the remaining deferred items (closed records, `assoc`/`keys`/`vals`
field-precise sinks).

## 2026-07-03 — check-project O(n²): FIXED via header-import redesign (26s → 5.8s @ 4000 files)

Followed the plan from the earlier investigation. `check_file`'s dominant per-file cost was
re-macroexpanding + re-evaling + re-**compiling** each file's `(defmodule … (:use …))` header (and
its per-file O(globals) `provide`/`require`/`%refer` scans + `*module-docs*` rebind). In a
whole-project check `project--ensure-loaded` already loaded every module, so that re-processing is
pure waste.

**Fix:** `types::check::setup_check_imports` — when checking a file, populate its import table
**directly from the header's `(:use)`/`(:alias)` clauses** instead of evaling the header. Mirrors
`defmodule`'s expansion + `%refer`/`%alias` exactly: `(:use mod)` refers mod's public (non-`--`) names,
`(:use mod :only [a b])`/`:refer` just those, `(:alias mod [:as short])` a prefix alias; a used module
that isn't loaded (bare-file check) is `require`d first. A module's public exports come from a new
count-keyed heap cache (`Heap::module_public_exports`); `known_ns` likewise (`known_ns_prefixes`) —
built once per check instead of an all-globals scan per file.

**Soundness (the recurring hot-reload concern):** the caches are **checker-only** (runtime `%refer`
still scans) and **count-keyed** — sound because Brood has no `undef`, so a permanent `def` only
increases the count (monotonic) and `%isolate` rollback restores the *exact* prior set, so a recurring
count ⇒ the same name-set; a hot-reload *rebind* keeps the set (cache stays valid) and an *add* bumps
the count (cache rebuilds). Regression test `checker_ns_caches_reflect_hot_reload_adds`.

**Bug caught in review:** the subset of `(:use mod :only [a b])` is a VECTOR; parsing it with
`list_items` (cons-only) silently returned nothing → import-all → masked real unbound errors. Fixed
with `seq_items` (vector + list); verified `nest check` now flags a name outside `:only`.

**Measured:** `nest check` 4000-file project 26s → 5.8s; 8000 ~350s(extrapolated) → ~18s. Gate: 655
tests (default + debug-assertions), clippy clean, all `(:use)` namespace + hot_reload + mcp tests pass.
A residual super-linear term remains (likely `project--check-unused-private`) — chasing next.

## 2026-07-03 — check-project fully LINEAR: the residual was more O(n²) `append`-in-fold sites

After the header-import redesign, a residual super-linear term remained. It was the SAME
variadic-`append`-in-a-fold O(files²) bug as the test runner, in THREE more places, all in
`std/tool/project.blsp`: file **discovery** (`project--collect-tests`/`-sources` built the path list
with `(append acc (list p))` per file), `project--unused-private-warnings`, and the `nest mcp`
`check-project-structured`. Each recopies the growing accumulator every file. Fixed with `cons` +
`append-two` (copy the small per-item list, share the accumulator tail).

**Measured:** `nest check` is now LINEAR — 1K→0.43s, 2K→0.80s, 4K→1.86s, 8K→3.12s (~0.4 ms/file);
8000-file project 87.6s → 3.1s (and ~350s-extrapolated → 3.1s, ~110×). Combined with the earlier
header-import redesign, the whole check-project quadratic is gone. Gate: 655 tests, brood-edit 725/725.

## 2026-07-05 — Type-system review (no bugs found) + intersection of arrows (ADR-116)

Reviewed the record/shape-types work (ADR-115) and the broader type-system module on
request. Adversarial-tested 12 record edge cases (duplicate field keys, nested
records, records nested in vectors, mixed keyword/non-keyword map keys, malformed
`(optional …)` wrappers, empty `(record)`, `(and (map K V) (record …))`
intersection, dynamic keys) — all sound, no crashes, no false positives. Grepped the
whole `types/` module for `unwrap`/`panic`/`TODO`/`FIXME`/`HACK` — nothing outside
test helpers. Full suite still green (173/173 types tests) after two days of
unrelated concurrent check-project O(n²) work. Found no code bugs, but did find and
fix three **stale roadmap entries** claiming features unshipped when `docs/types.md`
already documented them as done: type variables (`?A`, fully shipped),
`BROOD_CONTRACTS=1` (shipped), and singleton/literal types (the keyword half shipped
as ADR-105, only numeric/bool/string literals still deferred).

Then shipped **intersection of arrows** (ADR-116) — the roadmap's own "single
biggest expressiveness gap": `(and (int -> int) (bool -> bool))` used to parse fine
but silently widen to "any function" in `Ty::intersect` (two distinct known `Sig`s
treated as an unresolvable conflict). No new grammar needed — function intersection
types are the standard encoding of overloading, the same `(and …)` feature already
shipped, just applied to two distinct arrows instead of one arrow plus a flat tag.
`Ty` gained an `overload: Option<Arc<Vec<Sig>>>` refinement (2+ distinct sigs only —
a single one always collapses back to `arrow`, so every existing consumer is
untouched for the common case); a new `intersect_arrows` dedup-unions two sides'
candidate lists; `union` needed zero new code (the existing generic `merge_union`
already handles it); `is_subtype` generalizes (not parallels) the old single-arrow
check, conservative-but-sound per contract #5, mirroring ADR-115's
`record_fields_is_subtype`. New declaration-storage path (`Ctx::declared_overloads`,
mirroring `SigWithVars`) and call-site resolution (`resolve_overload_ret` in
`ctx.rs`) pick the matching arm's return type, union on ambiguity, widen to `ANY` on
no match — never fabricating a return type.

Verified the same two ways as ADR-115: 8 new unit/checker tests (types::mod.rs +
types::check.rs, 173→181), and a `nest check` diff across all of `std/`+`tests/`
with the new logic disabled vs. enabled — byte-identical, zero new warnings.

## 2026-07-05 — Intersection of arrows: cross-module resolution was missing (ADR-116 follow-up)

Follow-up to the same-day arrow-intersection work: the maintainer asked whether the
type checker is cross-file/cross-module, which led to checking whether the new
overload feature specifically was. It wasn't — `Ctx::declared_overloads` is
per-file (`check_file` allocates a fresh `Ctx` per file), so an overloaded sig
declared in one module was invisible when called from another, strictly worse than
a plain single-arrow `(sig …)` (which already crosses files via a separate
mechanism: `%register-sig` writes the raw type-expression form into a shared
heap-level store at load time, read back via `declared_heap_sig`'s `.as_arrow()`).
Fixed with no storage change — the heap store already held the opaque raw form
regardless of what it represented — by adding `declared_heap_overload` (mirrors
`declared_heap_sig`, extracts `.overload_sigs()` instead) and wiring it into the
same fallback positions `sig_of` already occupies in `expr_ty`'s call-form handling
and `callback_ret` (HOF callbacks); `check/walk.rs`'s argument-checking loop stays
untouched (same deferred scope as same-file overloads). Verified with a Rust test
that actually evaluates a declaration before typing a call against a fresh `Ctx`
(confirmed it fails without the fix, passes with it), and end-to-end with a real
two-file `nest new` project (`hello.blsp` declaring an overloaded `clamp`,
`main.blsp` calling `hello/clamp` via `(:use hello)`) — `nest check` correctly
flagged the genuine mismatch and stayed silent on the correct call. 182/182 types
tests green (up from 181).

## 2026-07-05 — Int-literal types: `5` as a type, first slice of ADR-105's deferral (ADR-117)

Continuing type-system work after the ADR-116 cross-module fix. ADR-105's one-line
deferral ("bool/int/string literals are the same machinery... deferred") undersold
the actual scope: `Value` has no `Ord`/`Eq`/`Hash` at all (float NaN blocks it
structurally), so a generic `BTreeSet<Value>` literal set across kinds is
impossible — and the existing `lit` field is hardwired to one tag (`KEYWORD_BIT`)
at every one of its ~6 call sites, so `(or :ok 5)` (two literal-bearing tags at
once) needs more than a drop-in. Resolved cleanly via a pattern already used twice
in this repo (`arrow`/`overload` on `FN_BITS`, `map_kv`/`fields` on `MAP_BIT`): a
third independent field, `lit_int: Option<Arc<BTreeSet<i64>>>` tagged a new
`INT_BIT` — since it's a different bit than `lit`'s, both compose on one `Ty` with
zero special-casing. Every `lit`/`KEYWORD_BIT` call site got a mechanically
parallel `lit_int`/`INT_BIT` block (union/intersect/negate/is_subtype/
is_disjoint/Display); grammar and runtime (`type-matches?`) each got one new
branch, no ambiguity risk.

Tried extending `Ty::of_value` to also make literal int *expressions* (not just
declared-sig types) into singletons — matching how keywords already work, so a
literal keyword argument at a call site gets static disjointness checking, not
just a runtime contract. Reverted: `of_value` feeds every literal int's inferred
type throughout the whole checker, so this changed unrelated misuse-warning
message wording project-wide (`"got int"` → `"got 5"`) and broke 7 pre-existing
tests. A materially bigger change than this slice's scope — reverted cleanly,
documented as a deferred follow-on needing its own design pass (`docs/type-int-literals.md`).

Verified the same two ways as records/arrows: unit tests mirroring every
keyword-literal test (render, union-exact, subtyping, disjointness, intersection,
plus a `(or :ok 5)` mixed-kind coexistence test), a checker-level test proving a
declared int-literal-set return type flows to callers, a `contract_test.blsp`
runtime block (50/50 passing, up from 46), and a `nest check` corpus diff with the
new parse arm disabled vs. enabled — byte-identical, zero new warnings. 189/189
types tests green (up from 182 before this session's overload work).

## 2026-07-05 — `nest check` parallelised across the worker pool (3–4× on huge projects)

`check-project` was linear (the earlier O(n²) sweep) but single-threaded: on a
100K-file project it spent ~17s in `check-files` + ~17s in the unused-private
parse pass, all on one core (profiled: the time is spread across the VM/GC running
the per-file Brood driver + the Rust checker, no single hotspot — the ideal shape
for parallelism, since each green process gets its own heap/GC/VM). Fanned both
passes across the scheduler pool via a small bounded driver in `std/tool/project.blsp`
(`project--pfold-files`): the same `monitor`/batch/collect discipline as the test
runner. `check-files` runs `check-file` per chunk and each worker **prints its own
warnings** (shipping the warning *list* back deep-copies it across heaps —
costlier than the check); unused-private is a **map-reduce** — workers parse a
slice once (killing the old double-parse: `all-sym-counts` + a per-file
`file-private-defs` re-parse) and return partial symbol-counts + private-defs,
which the driver merges. End-to-end `nest check`: **100K 41s→12.5s, 300K
161s→37s** (CPU ~800%). Correctness: parallel unused-private diffed byte-identical
against a reference sequential reimplementation on a 3000-file project with known
used/unused privates (1500/1500), and `nest check` on this repo is unchanged
(0 unused-private, same check-file warnings); 671/671 tests pass.

Two sizing lessons, both measured: (1) chunk to **`cores` big chunks, not many
small ones** — `check-file`'s first call on a fresh heap rebuilds the checker's
per-heap caches by scanning ALL globals (`known_ns_prefixes`, O(globals)); one
worker per handful of files makes that recur per chunk → O(files·globals) ≈
O(files²), which made a naïve 128-file-chunk version *slower* than sequential.
Sizing chunks to the core count caps the rebuild at ~cores times. (2) Group size =
`cores` (not a larger in-flight batch) — bounds peak live worker heaps, cutting
300K peak RSS from 5.75GB→3.85GB with no time cost.

**Kernel bug found + worked around (see KI-9):** the first cut passed the per-chunk
operation as a *closure* captured in the spawned worker's body. `spawn`'s move of a
body whose captured env holds a closure value intermittently corrupts that nested
closure's arity — a worker died with a bogus "fn: expected 0 arguments, got 1"
~1 run in 3, silently skipping its chunk's files. Worked around exactly as the
test runner does (ship only *data* — a keyword op — and resolve the operation
through the global table in the worker, never a shipped closure). The underlying
closure-deep-copy-on-spawn race is filed as KI-9 for a kernel fix.
