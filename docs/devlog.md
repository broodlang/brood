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

Two follow-on refinements shipped (95eac10): count only `--`-containing symbols in
the unused-private scan (the only names its verdict looks up — shrinks the shipped/
merged map from O(all symbols) to O(private refs); validated byte-identical vs a
full-symbol oracle), and walk the file tree once in `check-project` instead of twice.
Profiling the *real* (test-loaded, 0-warning) `nest check` then placed the residual
100K cost at ~0.8s overhead + ~4.5s unused-private (CST-parse-bound) + ~7s check-files
(genuine per-file type-checking) — both heavy phases already parallel, no serial
hotspot left. The remaining lever is not a faster from-scratch pass but *not redoing
unchanged work* → designed **ADR-119** (incremental check cache, `incremental-check.md`):
Phase 1 caches the pure CST passes by content hash (sound, no dep graph, ~40%); Phase 2
adds a dependency fingerprint + reverse-dep map for `check-files`. **Design only, not
built** — deferred per ADR-011 until a concrete large real project justifies it (the
only current driver is the synthetic 100K–1M stress projects). The advisory contract
(never rejects a runnable program) makes a stale-cache miss harmless, so Phase 2 may
over-invalidate freely — a safety margin a real compiler lacks.

## 2026-07-05 — Match exhaustiveness over literal-enum types (ADR-118)

Wired keyword-literal (ADR-105) and int-literal (ADR-117) types into their
motivating use case: `match` exhaustiveness. Initial scoping assumed this needed
a new `match`-clause parser (the checker has no correct view of `match`'s real
clause shape — `gradual_of_compound` assumes a wrong flat layout, dead code for
genuine `match` forms, left as-is), estimated at 2-3 slices. A much smaller design
was found by reading the actual compiler: `match` always compiles to a
`let`+`if`+`%eq` chain whose failure is `(throw [:match-error 'context target
'patterns])` — and that throw is *syntactically absent* whenever a catch-all
clause exists (an irrefutable clause skips straight to its body), and the full
list of tried patterns is quoted data sitting right there in the throw's 4th
vector slot. Combined with confirming that a `(%eq m lit)` guard's else-branch is
`then_only` (doesn't narrow `m`'s type — `guard_assertion`), the scrutinee's ctx
type at the throw is exactly its original declared type, unchanged. So the whole
feature is one new helper (`match_exhaustiveness_gap`, `check/guards.rs`) plus one
check in the existing generic `throw`-call path (`check/walk.rs`) — no new parser,
no new pass, no `Ty` change, and critically no reopening of the ADR-117
`of_value`/wording-churn question (this never touches literal-in-code inference,
only the declared scrutinee type).

`case` doesn't exist in Brood (confirmed vestigial in `eval/mod.rs`'s own error
message), so scope is `match`-only, as flagged before starting. Conservative by
construction: a non-literal pattern among those tried, or a mixed-kind/impure
scrutinee type, bails to no-warning rather than half-reasoning.

Verified with 6 new tests (missing keyword/int arm flagged, full coverage silent,
catch-all silent, destructuring-mixed silent, non-literal-enum-type silent) plus a
real 4-case demo through the `brood` CLI producing exactly 2 expected warnings, and
a `nest check` corpus diff (hook disabled vs. enabled) — byte-identical, zero new
warnings. 195/195 types tests green (up from 189).

## 2026-07-05 — Bool/string literals, generalized exhaustiveness, match redundancy (ADR-120/121/122)

Continuing type-system work: shipped all three follow-ons flagged as deferred after
the int-literal + exhaustiveness sessions. Note: these landed as ADR-120/121/122,
not 119/120/121 as originally planned — ADR-119 got taken by concurrent work
(the incremental `nest check` cache design) partway through this session; caught
and fixed via a careful ordered renumber across every file touched, double-checking
each shared doc (`roadmap.md`/`types.md`/`devlog.md`) didn't already carry the
*other* ADR-119's legitimate references before touching it.

**ADR-120 (bool/string literals):** mechanical repetition of the int-literal
pattern twice more (`lit_bool`/`lit_str`, `BOOL_BIT`/`STR_BIT`). String has one
real wrinkle — `Value::Str` is a heap handle, not inline data, so `lit_str` stores
owned `String` content (read via `heap.string(id)`) rather than the handle, or
two textually-identical literals wouldn't compare equal. Also revisited ADR-105's
"`false` isn't a literal type" guidance — that was scoped to avoiding `false`/`nil`
confusion in an *enumerated keyword* set specifically, not a technical limit; now
that bool literals are their own kind, both values are legitimate singletons.

**ADR-121 (generalized exhaustiveness):** the ADR-118 purity check required
*exactly* one bit (pure keyword or pure int); generalized to any combination of
the now-5 enumerable tags via one tag-subset test (`is_subtype` against the union
of all five, which — since that union carries no refinements — reduces to a plain
tag check). Declared/tested-set construction moved to string labels rather than
per-kind typed sets, sidestepping a combined Rust sum-type across 4 payload types.

**ADR-122 (match redundancy):** a different, independent problem — purely
structural on the compiled `if`/`%eq` chain, no scrutinee `Ty` needed. Reuses
`check_if`'s existing literal-guard recognition point, extracting the raw literal
value (not just its `Ty`) and scanning forward for a duplicate test on the same
symbol. Genuinely general — fires on a hand-written same-symbol `%eq`-chain too,
not just `match`-generated ones.

Verifying the redundancy check against the whole corpus surfaced one real finding,
not a bug: `tests/pattern_matching_test.blsp`'s test **"first matching clause
wins"** deliberately writes `(match 1 (1 :first) (1 :second) (_ :z))` to prove
runtime clause-priority — a true positive, correctly flagged, left as-is (the test
still passes; advisory warnings never gate). Took real digging to confirm this
wasn't a bug in the new check: bisecting a 367-line test file by raw line-count
cuts gave misleading results (truncating mid-form corrupts parens and changes
what's even parseable) — the reliable technique was removing whole top-level
`describe` blocks from the *full* file and re-checking, which correctly isolated
the single deliberately-duplicated-literal test.

Also answered two side questions during this session: whether recursive/self-
referential map or record types could infinite-loop the checker (no — `Ty` values
are immutable and built compositionally from finite source text with no named
type-alias resolution mechanism, so no cycle can ever be constructed; a 3-level
nested `(map string (map string (map string int)))` checks and runs instantly),
and confirmed via `nest check`/`nest test` bisection that the false-positive
investigation above was fully resolved before shipping.

214/214 types tests green (up from 195 at the start of this round). Full corpus
`nest check` diff clean apart from the one documented true positive.

## 2026-07-05 — Revised direction: pursue full Elixir-parity soundness (ADR-123, design only)

Course-corrected the type-system roadmap: the "map of distance to Elixir" gap
list (`docs/roadmap.md`) was framed as "reference, not a target" and two items
— pervasive static soundness/gating, and wiring `dynamic()` into actual gating
— were marked ✋ deliberately-not-pursuing. That framing was written by a prior
session, not requested; the actual direction is to burn the whole gap list
down, soundness included.

The apparent blocker was that gating on global `def`/`defn` types looks
incompatible with Erlang-style hot reload (ADR-013 — a `def` rebinds a global
unconditionally, visible to every process sharing the runtime on its next
lookup). Traced the compiler/JIT to check the actual constraint before
designing around it: runtime type safety is **already fully independent of the
static checker** — every operation does a real runtime tag check regardless of
what was statically proved, confirmed by `types/check.rs` and
`eval/compile.rs` having zero data flow between them. So a reload that breaks
a prior proof can't crash anything; worst case is a catchable runtime type
error, same as any dynamic-typing mismatch today.

That unlocks the design in **ADR-123** / [`type-soundness-reload.md`](type-soundness-reload.md):
treat soundness as re-asserted per `def` rather than proven once forever —
globals get a real trackable type, the checker records which call sites depend
on it, and every reload triggers a targeted re-check of those dependents,
surfacing fresh warnings without ever blocking the reload. A hard reject stays
possible only for batch/CI tooling (a future `nest check --strict`), never the
live image. Design only — no runtime code yet; the dependency index, the
reload hook, and precise invalidation are the remaining work, deferred per
ADR-011 until picked up. `docs/roadmap.md`'s framing, the gap-list markers, and
the `CLAUDE.md`/`docs/types.md` "checking never rejects a runnable program"
invariant are all flagged as due for revision in lockstep with whichever slice
of this actually ships.

## 2026-07-05 — ADR-123 slice 1: cross-module value-type sigs (ADR-124)

Picked up the first concrete piece of ADR-123's design: a per-global type is
only useful for a future dependency index if it's actually *visible*
wherever the global is referenced, not just within the file that declared it.
Arrow sigs already had this (`sigs::declared_heap_sig`, reading
`%register-sig`'s heap-wide store keyed by the module-qualified name); plain
value-type sigs (`(sig x T)`, non-arrow) didn't — `walk::gradual_of`'s global-
reference branch only consulted the file-local `Ctx::declared_value_ty`,
scanned from the current file's own un-expanded forms.

Added `sigs::declared_heap_value_ty` (same heap store `declared_heap_sig`
reads, non-arrow branch instead of `.as_arrow()`) and wired it as a fallback
in two places: `gradual_of`'s reference branch, and — found while writing the
cross-module test, since proving an actual assignment warning needs both
sides visible — `check_def`'s own "does this def's value match its declared
type" gate, which had the identical file-local-only gap for the *name being
defined*, not just the value referenced.

New test mirrors the existing arrow cross-module test's technique (real
`Interp`, `eval_str` the declarations so `%register-sig` really populates the
heap, then check a bare form against an empty `Ctx`). Full corpus `nest check`
diff clean (91 warnings before and after, byte-identical). 216/216 types tests
green (up from 215).

This is a precondition for ADR-123's dependency index, not the index itself —
the reload hook and dependency tracking are still fully undesigned-in-code.

## 2026-07-05 — Merge fallout: ADR-124's new heap read bypassed Phase 2's recorder

Pushing ADR-124 collided with a separately-developed branch landing at the
same time: **ADR-119 Phase 2** (the incremental `nest check` cache), which
introduces a strict new rule for everything under `types/check/` — every read
of global state must go through a `deps::obs_*` wrapper, or the cache's
dependency fingerprint can't see what a file's check actually depended on and
may serve stale (wrong) warnings after an unrelated edit. The merge was
textually clean (git's 3-way auto-merge, no conflict markers), but Phase 2's
branch had updated the two *pre-existing* heap-read functions
(`declared_heap_sig`/`declared_heap_overload`) to route through
`deps::obs_declared_sig_value` — my *new* one, `declared_heap_value_ty`, was
written independently on the other side of the fork and still read
`heap.declared_sig_value` directly. Caught it by reading `check/deps.rs`'s own
doc comment ("the ONLY sanctioned reads of global state") right after the
merge and grepping for what still called the heap directly.

Fixed the read, then built a regression test proving it actually matters:
`cross_module_value_sig_dependency_is_captured_for_incremental_cache` isolates
`check_def`'s def-target gate specifically (a global's value-sig declared only
on the heap, never referenced anywhere in the checked file except as the def
target — so nothing else, like the unbound-symbol check, would incidentally
record it via a different `obs_*` call). Verified the test's bite by reverting
the fix and confirming it failed (unchanged fingerprint after editing the
sig), then restoring it and confirming green. An earlier version of this test
(editing a *referenced* global's sig, not a pure def-target's) passed with the
bug still present — a false sense of coverage, since that symbol got recorded
via the ordinary unbound-symbol check regardless of my fix. Worth remembering:
a dependency-tracking regression test needs a dependency that's *invisible to
every other path*, or it doesn't actually isolate the one you're fixing.

359/359 unit tests, corpus `nest check` unchanged (91 warnings).

## 2026-07-05 — ADR-123's Step 2 turned out to already exist

Went looking to build ADR-123's next slice — the reverse-dependency index
(`global → dependent call sites`) the design called for — and, before writing
any code, read what ADR-119 Phase 2 (merged into `main` the same day, right
before this) actually shipped in `check/deps.rs` and `docs/incremental-check.md`.
It solves the identical underlying problem — "did anything this file's check
depended on change?" — for a different stated reason (skipping re-check of
unchanged files in the batch `nest check` CLI), but via a strictly simpler
mechanism than the one this design proposed: no reverse index is ever built or
maintained. Instead each file's dependency facts are recorded once
(`check-file-deps`), and on a later run the fingerprint of those *same*
recorded facts is cheaply *re-observed* against the current image
(`check-deps-fp`) and compared — a mismatch means something changed, with no
need to know what, or to have ever built a `global → dependents` map at all.

Rewrote `type-soundness-reload.md` and the ADR-123/roadmap references before
writing any Step-2 code against the now-stale plan: the reverse-dependency
index is struck from the design as unnecessary, not deferred. What's actually
left of ADR-123's "hard part" shrank to one real question — Phase 2's cache is
consulted only by the batch CLI today; there's no live-session trigger that
re-runs it in response to a `def` happening in a running REPL/eval session.
Deciding *where that trigger lives* (file-save via `nest run --watch`'s
existing watcher, a REPL-level hook, or purely LSP-request-driven) is the
entire remaining scope — not a data structure to design or build.

No code changed this round — design/roadmap/ADR-123 docs only, keeping the
"design only, not built" status accurate before more implementation work
lands on top of it.

## 2026-07-05 — ADR-125: `nest run --watch` re-checks on reload

Shipped ADR-123's one remaining open question: the live-session trigger.
Gave `std/tool/reload.blsp`'s `reload-on-change` (and its internal
`reload--loop`/`reload--dir-loop`) an optional `on-reload` callback, invoked
after every *successful* reload with its own errors caught separately (a
broken callback can't take the watcher down, same contract as a broken
save). `reload.blsp` itself stays project-agnostic; `nest run --watch`'s
generated glue (`crates/nest/src/main.rs`) supplies the actual policy —
`(fn (_p) (project/check-project-sources))` inside a project, `nil` outside
one.

Planned to route every callback through a dedicated serializing process
first, since ADR-119 Phase 2's dependency recorder was thread-local at the
time and a directory watch spawns one reload process per file — concurrent
`check-file-deps` calls could clobber it. Paused mid-design when a concurrent,
independent refactor (landing in the same session) moved the recorder onto
`Heap` itself (per-process, not per-OS-thread), making the hazard disappear
at the source. Waited for that refactor to compile before finishing, rather
than build a workaround for a problem about to be fixed underneath it —
confirmed via `project.blsp`'s new `project--pcheck-deps`, which now runs
`check-file-deps` across the worker pool in parallel.

Verified end-to-end, not just unit-tested: scaffolded a real project via
`nest new`, ran `nest run --watch src` in the background, edited a function
body to introduce a real call-site type mismatch while it was running, and
watched the warning appear live with no restart — then fixed it and watched
the warning clear on the next reload.

Two detours while building `tests/reload_watch_test.blsp` worth remembering:
(1) the first draft timed out because two `spit` writes with no gap landed in
the same millisecond — `file-mtime`'s resolution — which the watcher
correctly read as "no change"; not a watcher bug, a race in the test, fixed
with a small `(sleep 100)`. (2) a variable named to echo the enclosing
module's own name (`reload-watch-test--val`) got auto-qualified the moment
*any* literal `def` for it existed anywhere in the same `defmodule`-wrapped
file — even one added temporarily deep inside a debug `spawn` — because the
qualification pre-scan doesn't care about nesting depth. Renamed away from
the collision and read the dynamically-`load`ed global via `(eval 'sym)`
rather than a bare reference, since a bare reference is exactly what the
static unbound-symbol checker (correctly) can't resolve for a name a runtime-
loaded temp file will define — fixed both the qualification confusion and 6
new corpus warnings in one move.

Also surfaced, unrelated to this feature: `(sig fname (A -> B))` declared
inside a `defmodule` block doesn't seed `check_def`'s body-vs-declared-
return-type check — Pass 2.5 records the sig under the bare name, but the
expanded `defn` target is the qualified name, so the two never meet. A real
false-negative (silent, not over-warning), logged in
`docs/type-annotations.md`'s new "Known gap" section, not fixed here — out of
scope for this slice.

359/359 unit tests, corpus `nest check` unchanged (91 warnings).

## 2026-07-05 — ADR-126: fixed the defmodule arrow-sig seeding gap

Came back and fixed the gap ADR-125 surfaced. Same shape as ADR-124's fix,
one namespace over: `check_def`'s seeding lookup (`ctx.declared_sig(name)`)
now falls back to the heap-wide `declared_heap_sig(heap, name)` when the
file-local `Ctx` (keyed by the bare name Pass 2.5 recorded from un-expanded
source) misses — exactly what call-site checking (`sig_of`) already had.

Verified with the revert-then-confirm technique this session settled on for
every checker change: added `defmodule_declared_arrow_sig_seeds_return_type_check`,
confirmed it fails with the fix reverted (proving it actually isolates the
bug, not just exercises an already-working path), then restored the fix and
confirmed green. Full `nest check` corpus across `std/` + `tests/` stayed at
91 warnings before and after — the mismatched `defmodule` + `sig` + `defn`
pattern this fixes doesn't occur anywhere in the current committed source,
so this closes a real gap without any pre-existing bugs to triage.

360/360 unit tests, corpus `nest check` unchanged (91 warnings).

## 2026-07-05 — `nest check --strict` was already built

Went to build the last piece the roadmap listed as unbuilt for ADR-123 — a
`nest check --strict`/`BROOD_CHECK_STRICT=1` flag gating CI on any warning —
and checked the actual behavior first rather than assuming the docs were
current. `cmd_check` in `crates/nest/src/main.rs` already exits 1 on any
nonzero warning count, unconditionally, with no flag involved — confirmed
directly (a clean file exits 0, a file with one warning exits 1). This
predates the whole ADR-123 thread; the design doc's "still unbuilt" framing
was simply wrong when written. Corrected `type-soundness-reload.md`,
`roadmap.md`, and the ADR-123 entry in `decisions.md` — ADR-123 is now fully
shipped with nothing left open. No code changed.

## 2026-07-05 — ADR-127: `&optional` in `(sig …)` arrow grammar

Picked up the roadmap's "richer `(sig …)` type-exprs (rest/optional params,
nested generics)" item. Probed all three parts before writing any code:
`&` rest params and nested type variables (`(list ?A)`) both already worked
— rest via the existing `parse_arrow` marker, nested generics via
`SigWithVars`/`SigTerm` from an earlier session (type-variables.md slices
1–2). `&optional` was the actual gap, and probing it found something worse
than "unchecked" — `(sig g (int &optional string -> int))` silently dropped
the *entire* sig (zero warning at all, not even for an obviously wrong call),
because `parse_arrow` had no case for the `&optional` symbol and the whole
`Option`-chained parser just returned `None`.

Extended `Sig` with an `optional: Vec<Ty>` field, empty in every existing
constructor (checked: zero behavior change for every current caller).
Routed everything through the one existing choke point, `Sig::param(i)`
(params → optional → rest), so call-site checking and subtyping needed no
separate optional-awareness — just a fallback clause in one function.
`parse_arrow` now parses `params... &optional opt... & rest -> ret` in any
combination, mirroring a closure's own param shape.

Generalized `Sig::is_subtype`'s arity gate from an exact equality check to
an arity-range comparison — worked out the algebra by hand and confirmed it
reduces to the exact original check when `optional` is empty on both sides,
so no existing arrow-subtype comparison in the corpus could change. Also had
to fix `check_fn_seeded` (the same seeding path ADR-126 touched) twice more:
its filter gated on exact param-count equality (would've silently rejected
seeding for any optional-having sig), and its per-position loop read
`s.params.get(i)` directly instead of `s.param(i)` (would've never seeded an
optional position even once the filter let it through).

The one real design decision, not just plumbing: an optional param seeds the
body as `T | nil`, not exact `T`, via a plain `bind` rather than
`bind_sig_param` — because it may genuinely be absent, and seeding it exact
would make a defensive `(if (nil? b) …)` look like dead code to a lint that
trusts a sig-typed param's declared type as precise. Verified directly: a
defensive nil-check stays silent, using the param unconditionally as
non-nil still warns.

While verifying, hit a stash scare worth recording: ran `git stash` to
isolate whether a failing test predated this session's changes, without
first checking that the working tree also held a *different* concurrent
session's in-progress, uncommitted edits (a `deps.rs` refactor). The stash
swept both up together. Caught it immediately from the diff shown back and
`git stash pop`'d right away — nothing lost, but a reminder to check
`git status` for whose changes are actually sitting there before running
any stash/reset, not just before the more obviously destructive commands.
The failing test itself (`cross_module_value_sig_dependency_is_captured_for_incremental_cache`)
turned out to collide with that other refactor's new `dep.own` exclusion
filter — confirmed via the stash-and-restore, not fixed, since it's not
this ADR's code to change.

New test `optional_sig_params_parse_and_check` passed every assertion on
the first run — call-site type + arity checking, both nil-widening
directions, `&optional` combined with a trailing rest, and the malformed-
marker-order case, all in one test. 360/360 unit tests green (the one
pre-existing unrelated failure aside), `nest check` corpus unchanged (91).

## 2026-07-05 — ADR-128: tuple / positional product types

Picked up the last concrete Elixir-parity item: Brood had no way to type a
fixed-arity, per-position vector shape at all — only the uniform `(vector
E)`. Followed records' exact precedent: a fifth structural refinement on
`Ty` (`tuple: Option<Arc<Vec<Ty>>>`), tagged to `Vector` alone, layered on
top of the existing runtime value with no new `Value` kind. Mechanical parts
(struct field, constructors, `parse_type` grammar, `Display`) went fast;
`union`/`intersect` reused the existing generic `merge_union`/
`merge_intersect` helpers unchanged, since `Vec<Ty>: PartialEq` was already
sufficient — no bespoke merge logic needed there.

The two places that needed real thought, not just plumbing:

- **`Ty::elem_ty()` becomes the fallback choke point.** Made it derive a
  union-of-positions type when a `Ty` has `tuple` but no plain `elem` — this
  single change is what makes `tuple<int,string> <: vector<int|string>` (and
  every `first`/`nth`/`rest` consumer of `elem_ty()`) work for free, without
  hunting down every individual call site. Changed `elem_ty`'s return type
  from `Option<&Ty>` to owned `Option<Ty>` to make the synthesis possible —
  turned out to *simplify* most callers, since they were already
  `.elem_ty().cloned()`.
- **`is_disjoint` (not `is_subtype`) is what the "argument N expects X, got
  Y" warnings actually consult**, and it's tags-only except for a few
  precise special cases (the keyword/int/bool/string literal sets). Added a
  genuinely sound tuple-vs-tuple case there too — different arity, or any
  disjoint position, is provably disjoint — mirroring those existing cases.
  Missing this piece was the reason the first end-to-end probe of a
  mismatched tuple *argument* silently passed even though the type
  machinery was otherwise correct.

**The literal-inference change was the real risk in this slice**, and it's
the part I was most conservative about going in: a `[a b c]` vector literal
now infers its exact positional shape (`tuple_of`) instead of widening to a
uniform `vector_of(union)` — a behavior change to inference that's been
stable for a while, not just new grammar nobody was relying on yet. Reasoned
through why it should be safe *before* touching it (a tuple is already a
subtype of the corresponding uniform vector via the `elem_ty()` fallback, so
nothing that passed before could start failing), then verified: full `nest
check` corpus diff across `std/` + `tests/`, byte-identical, 91 warnings
before and after.

Added position-aware `first`/`second`/`third`/`last`/`nth` (a literal index
resolves to the exact position, not the coarse union every other element
access still gets) and a `tuple` case in `type-matches?` for `sig!`/
`BROOD_CONTRACTS=1` runtime enforcement, mirroring `record`'s case exactly.

**A real workflow gotcha, cost real time this round:** the incremental
`nest check` cache (ADR-119) stamps itself with a git-SHA build-id, which
doesn't change across uncommitted local rebuilds. Several times mid-session,
a genuinely-fixed behavior (confirmed correct via `cargo test`'s
in-process `file_warnings()`, which never touches this cache) still showed
the *old*, wrong result through the `nest check` CLI after a real rebuild —
because the cache didn't know the checker's logic had changed, only that the
file content and build-id hadn't. Traced it by comparing the in-process test
result against the CLI result for the identical source and noticing they
disagreed; `BROOD_NO_CHECK_CACHE=1` confirmed the diagnosis and became the
standard for the rest of this session's CLI-level verification. Worth
remembering for any future checker-logic iteration: the cache is safe for
normal use (a real commit changes the build-id), but actively misleading
while iterating on uncommitted checker changes.

New test `tuple_sig_params_parse_and_check` covers parsing, call-site
argument + arity mismatch, all four positional sinks, declared-return-type
mismatch, and the tuple-satisfies-uniform-vector case — passed every
assertion on the first write. Plus 5 new `sig!` contract tests. 362/362 unit
tests, 2605/2605 whole-project test suite, `nest check` corpus unchanged
(91, verified with the cache genuinely disabled).

## 2026-07-05 — ADR-129: fixed the check-cache staleness bug for real

Came back and fixed the workflow gotcha flagged when ADR-128 shipped: `(build-id)`
— the incremental check-cache's staleness stamp — was purely git-sha-based,
baked in at compile time via a `build.rs` that only reruns on `.git/HEAD`/
`.git/refs/heads` changes. An uncommitted local rebuild never produces a new
stamp at all, so the cache kept serving warnings from the *previous* binary.
Added a second component, `binary_stamp()`: the running executable's own
mtime, read at runtime via `std::env::current_exe()`, cached once per
process. Correct by construction — changes on literally any rebuild, for any
reason — rather than trying to track which source paths matter to which
cache. One consumer in the whole codebase (`project--cache-stamp`), so low
risk.

Verified properly, in two stages. First, confirmed the fix itself: touched a
file with zero content change, rebuilt, confirmed `(build-id)` changed
anyway (proving it's tied to the rebuilt binary, not file content). Then did
a real round-trip against `nest check`'s actual cache: populated it with a
genuine warning, disabled the check that produces it, rebuilt without
committing, confirmed `nest check` (no env var) correctly showed it gone;
restored, rebuilt, confirmed it correctly came back.

Caught my own mistake mid-verification, worth recording: the first attempt
to "disable" the check used `if false { } else if let Some(s) = sig { … }`
— which is a no-op (`if false {A} else if COND {B}` is just `if COND {B}`).
When the warning still appeared, the fix I'd *already independently
confirmed* via the plain touch-test told me the bug was in my test
methodology, not the fix — so I went looking for what I'd gotten wrong
instead of doubting a change I'd already verified a different way. Fixed it
to a genuine `false && …` disable and the round-trip worked correctly both
directions.

While re-establishing the corpus baseline with the cache now genuinely
reliable, found that ADR-128's "91 warnings, unchanged" claim was itself
measured through the very staleness bug this ADR fixes — the true,
cache-independent count (confirmed via a clean worktree at the pre-ADR-128
commit, and again with the cache directory deleted entirely) is 93, not 91.
The 2-warning gap is not a tuple regression, though: `tests/bytes_test.blsp`
already had that exact "expects bytes, got vector<int>" warning before
ADR-128 (confirmed present at the parent commit with that wording) — the
literal-inference change only reworded it to `(tuple int, int, int)`, same
warning, more precise text. Corrected the record in ADR-128's entry rather
than leaving a wrong number to be taken at face value later.

362/362 unit tests unaffected throughout — this bug and its fix live
entirely in the CLI/cache layer; `cargo test`'s in-process checking never
touched it.

## 2026-07-06 — checker false-positive sweep (bytes seqable, gensym lint exemption, proc-send)

Ran `nest check` across the whole tree and fixed three genuine checker
false-positive classes (the full suite was already green — 2605/2605, and
`docs/known-issues.md` has no open items; KI-9 did not reproduce):

- **`bytes` is seqable/countable.** `count`/`length`/`first`/`rest`/`every?`/
  `map`/… all iterate a `bytes` value's octets at runtime, but the checker's
  `seq`/`countable` domains omitted `Tag::Bytes`, so every such call on bytes
  warned. Added `Tag::Bytes` to the two curated `seq`/`countable` consts in
  `types/check/sigs.rs` and to the builtin `seq` const in `builtins/mod.rs`
  (the domain for first/rest/nth). ~21 warnings gone.
- **Gensym temporaries no longer linted "unused."** A macro expansion
  (match / pattern lowering) can attach its call-site position to the `let` it
  generates, so the unused-binding lint's position-based "compiler-generated"
  exemption missed them and flagged names like `m__1380`. Added a name-based
  exemption in `check_let` (`walk.rs`): a `<prefix>__<digits>` gensym name is
  skipped (consistent with the existing `_`-prefix exemption; the lint already
  errs toward false negatives).
- **`proc-send` accepts bytes.** Its own doc says data may be a string *or* a
  bytes value, but the checker sig typed arg 2 as `string`. Widened to
  `str | bytes`.

Also removed a dead `use crate::eval;` in `eval/macros.rs` (the code uses the
fully-qualified path). Added regression coverage in `types/check.rs`
(bytes-seqable stays silent; gensym-named binding exempt but a hand-written
`my__thing` still flagged). 219/219 checker unit tests pass; full in-language
suite 2605/2605.

Residual `nest check` warnings are all intentional or build-artifact, not
bugs: the documented non-tail-recursion lint (torture/`pm-fac` tests);
deliberate shadowing tests (`(let (= …) …)`, `(let (list … map …) …)`);
adversarial negatives (a bytes pattern matched against a non-bytes value emits
guarded `byte-at`/`byte-length` the checker can't see past — the standing
`bytes_test.blsp` "(tuple int,int,int)" warning); a `bound?`-guarded reference
to the debug-only `%blob-ptr`/`%blob-strong-count` primitives, which surfaces
**only in a release build** (they're `#[cfg(debug_assertions)]`, so the dev
build the invariant is validated against sees 0); and ~26 genuinely-unused
leftover test bindings (`(let (w (spawn …)) …)` handles bound for effect) —
correct advisory lints, fixable by `_`-prefixing, left as-is for now.

## 2026-07-06 — Checker: float-contagion arithmetic (the last precise-body-inference slice)

Closed the remaining catchable half of "precise body inference" (roadmap Step 5+;
the int-closed half shipped 2026-06-30). `numeric_call_ty` (`types/check/guards.rs`)
gained a **float-contagion** rule alongside the existing int-closed one: `+ - * /`
with any operand *provably* `⊆ float` yields `float` (IEEE/tower contagion —
`(+ 1 2.0)` → `3.0`), and `sqrt`/`sin`/`cos`/`tan` are always-float even for a
whole-number argument (`(sqrt 4)` → `2.0`). Both results stay `⊆ number`, so they
can only sharpen — never widen — a type. Because `float` is disjoint from `int`,
the sharpened result flows straight into the *existing* return-type disjointness
warning with no new logic: `(sig f (int -> int)) (defn f (x) (+ x 1.5))` now warns
"declared return type int but the body yields float".

The complementary *merely-wider* case stays deferred, correctly: `(/ x 2)` on two
ints is genuinely `number` (`(/ 6 2)` → `3`, `(/ 5 2)` → `2.5`), so `/` is in the
contagion group but NOT the int-closed group, and an all-int `/` pins to neither →
defers to `number`. Pinning it to `int` would be a lie; warning would false-positive
on the int-valued runs. That residue needs occurrence/range analysis and stays out
(ADR-011).

**Verified.** New regression test `precise_body_inference_float_contagion` (4 warn
cases + 4 sound-defer cases); the four `precise_body_inference_*` tests pass. Gate:
a full-corpus `nest check` diff against a HEAD worktree baseline came back
**identical** — zero new or removed warnings across all of `std/` + `tests/`.

## 2026-07-06 — `nest check` to zero: checker false-positive sweep + `check-allow` directive

Drove `nest check` from 54 warnings to **0**. The 54 split into checker imprecision
(fixed properly) and correct-lints-on-deliberately-written-test-code (opted out with
a new directive).

**Checker fixes (54 → 27), all genuine false positives / imprecision:**
- **Debug-only primitives** (17): `%blob-ptr`/`%blob-strong-count`/`%force-panic`
  are `#[cfg(debug_assertions)]`, so a release `nest check` saw every guarded test
  reference as unbound. The checker now knows their names regardless of build config
  (`is_debug_only_primitive` in `walk.rs`).
- **Destructuring pattern-let** (2): `(let ((a b) rhs) (+ a b))` never bound the
  pattern's symbols → `a`/`b` flagged unbound. `check_let` now binds every symbol
  leaf of a destructuring binder (`pattern_syms`).
- **Deliberate shadows** (5): `(let (list …) …)`/`(let (= …) …)`/`(let (*dt* …) …)`
  — an unused binding that *shadows* a global/curated/file-global is a scope-isolation
  test, not a leftover, so the unused-`let` lint now exempts it (`ctx.is_file_global`
  added for the file-global case).
- **`string-contains?` nil** (1): its sig said `string`, but `index-of` treats a nil
  haystack as empty (→ false), so arg 1 is really `string | nil` — sig widened.
- **`bytes?` occurrence typing** (2): a bytes-pattern match lowers to
  `(if (bytes? m) … (byte-length m) …)`, but `bytes?` wasn't in `Ty::tested_by`, so
  the guarded `byte-length`/`byte-at` were flagged against a non-bytes scrutinee.
  Added `bytes? → Bytes` (narrows everywhere now).

**`check-allow` directive (27 → 0).** The remaining 27 (26 non-tail-recursion JIT
torture cases + 1 redundant `match` clause) are the checker being *correct* — the
test code deliberately has that shape to exercise it. Comments can't suppress (the
reader strips them pre-check), so added a form-level directive:
`(check-allow :category form…)`, a prelude macro expanding to a `%lint-allow` marker
that survives macroexpansion and is a pure runtime no-op (yields the wrapped body).
The checker reads it: `recursion.rs` skips a `:non-tail-recursion` subtree, and a
`SUPPRESS_*` bit threads through `Ctx` so `check_if`'s redundancy lint declines on
`:unreachable-clause`. An unrecognised category suppresses nothing (no silent
blanket opt-out). Applied to the 5 torture/redundancy test sites.

**Verified.** `nest check` = **0 warnings** project-wide. New regression test
`check_allow_suppresses_targeted_lints` (suppresses the right category, not a
mismatched one, for both lints). Gates: Rust lib 364/364, `types::` 221/221, the
full in-language suite **2605/2605**, every touched test file green (the `check-allow`
wrapper preserves `defn`/`match` runtime semantics — confirmed by the passing tests).

## 2026-07-06 — Arrow-intersection argument check (ADR-116 completion)

Finished the one piece ADR-116 explicitly deferred: flagging a call whose
arguments match **no** arm of a declared overload. ADR-116 shipped the
input-dependent *return* type (`resolve_overload_ret` in `expr_ty`); the argument
side needed "a second hook in the separate arity/argument-checking loop," which
`check_into` now has. When a callee has no single `sig` but a declared overload
(`ctx.declared_overload` / `declared_heap_overload`), `overload_arg_mismatch`
(`walk.rs`) flags the call — but only when *every arity-relevant arm is ruled
out*, an arm being ruled out only when a *known* argument is provably **disjoint**
from its parameter. False-positive-free by the same discipline as the single-sig
loop: unknown/`NEVER` args never rule an arm out, disjointness (not subtyping) is
the test, and a pure arity mismatch (no arm with a fitting arity) defers to the
arity check instead of double-reporting.

`(f "hello")` / `(f :nope)` against `(and (int -> int) (bool -> bool))` now warn
"f: no overload clause accepts these arguments"; `(f 5)`, `(f true)`, and an
unknown-typed `(f y)` stay silent.

**Verified.** New test `overload_call_matching_no_arm_is_flagged`; `types::`
222/222; clippy clean; **`nest check` still 0 warnings** across `std/` + `tests/`
(no new false positives).

## 2026-07-06 — Path narrowing: occurrence typing through `(get base :key)`

First slice of the roadmap's "narrowing through non-variable expressions": a
type-predicate guard over a record-field path now narrows the *path*, not just a
bare variable. `(if (int? (get r :age)) (string-length (get r :age)) …)` types
`(get r :age)` as `int` in the then-branch — catching the `string-length` misuse
that the bare-symbol-only occurrence typing missed (and `¬int` narrows the else
branch, since a type predicate is biconditional).

Pieces: `Ctx` gains `path_types: (base, key) → Ty` with `narrow_path`/`path_ty`
(and a `bind` that drops a base's paths when it's rebound); `guards.rs` gains a
`PathGuard` + `path_guard_assertion` recognising `(pred? (get base :key))` and its
`(not …)`; `expr_ty`'s `get` rule consults the narrowing *first* (it's the more
specific type the guard proved); `check_if` layers the path narrowing on top of
the existing symbol narrowing. Sound under Brood's immutability — `base` and the
pure `get` can't change between the guard and the use, exactly like the
bare-symbol case. Scoped to a keyword key + bare-symbol base (the record case);
a computed base / nested path is the deferred general form.

**Verified.** New test `path_narrowing_through_a_record_field_guard` (the
string-length catch + four consistent/unguarded/else-branch no-warn cases);
`types::` 223/223; clippy clean; **`nest check` still 0 warnings** across
`std/` + `tests/`.

## 2026-07-06 — Path narrowing, general form: nested keyword-`get` chains

Generalized the path-narrowing slice from a single `(base, :key)` to a base plus
an **arbitrary-depth chain of keyword keys**, so a nested access narrows too:
`(if (int? (get (get cfg :db) :port)) (string-length (get (get cfg :db) :port)) …)`
now catches the misuse, exactly like the single-level case. A new `get_path`
(`guards.rs`) peels a `(get (get … :k1) … :kn)` chain to `(base, [:k1 … :kn])`
base-outward (a bare symbol → empty path, so `path_guard_assertion` still defers a
plain variable to `guard_assertion`). `Ctx::path_types` is rekeyed
`(Symbol, Vec<Symbol>)`; `narrow_path`/`path_ty` and the `expr_ty` `get` rule take
the full chain (the use site peels `map_arg` and appends its own key). A
*different* path than the one narrowed is unaffected (path-specific, verified).

Still deferred: a computed (non-keyword) key or `nth`/tuple-index path, and
refining `base`'s own record type so the narrowing flows into a function call.

**Verified.** Extended `path_narrowing_through_a_record_field_guard` (a nested
warn case + a different-nested-path no-warn case); `types::` 223/223; clippy
clean; **`nest check` still 0 warnings**.

## 2026-07-06 — Path narrowing flows into calls: base record refinement + record disjointness

The deeper half of path narrowing: a guard now refines `base`'s **own type**, not
just the exact field path, so the narrowing flows when `base` is passed to a
function. `(if (int? (get r :age)) (f r) …)` refines `r` to the open record
`{age: int}` in the then-branch (built inner-out for a nested path), so calling
`f` — declared `((record :age string) -> …)` — is flagged. Sound: a true guard
proves the whole `get` chain is present and typed, so `base` is a record with that
(nested) field; refinement is then-branch only (the else-branch can't prove the
field is present).

This surfaced the missing link — `Ty::is_disjoint` compared only tags for records
(both `Map`-tagged → never disjoint), so a record type never flagged at a call.
Added a sound **record-disjointness** rule (mirrors the existing tuple rule): two
records are disjoint when they both constrain a field, it's *required* on at least
one side, and the field types are disjoint. Optional-on-both or single-side fields
never manufacture disjointness (open records). Updated the old
`record_is_disjoint_only_on_tags…` test — which pinned the previous tags-only
behavior — to `record_disjointness_needs_a_required_conflicting_field`.

**Verified.** New test `path_narrowing_refines_base_record_type_into_calls`
(the `{age:int}`-vs-`{age:string}` call catch + matching/unguarded no-warn);
`record_disjointness_needs_a_required_conflicting_field` (4 cases); Rust lib
**366/366**; clippy clean; **`nest check` still 0 warnings** (the new record
disjointness added no corpus false positives).

## 2026-07-07 — Path narrowing: index paths (`nth`/`first`/`second`/`third`)

Completed path narrowing by generalizing the path key from a bare keyword
`Symbol` to a `PathKey { Field(Symbol) | Index(usize) }`, so fixed-index accessors
narrow like field access and mix freely with `get` chains:
`(if (int? (nth (get r :items) 0)) (string-length (nth (get r :items) 0)) …)` is
caught. `get_path` became `path_of`, recognising `get` (keyword key), `nth`
(literal non-negative index), and `first`/`second`/`third` (0/1/2); `last` and any
computed (non-literal) key/index are excluded — the latter is statically
unpinnable, so there's nothing left to narrow. The per-accessor path check that
lived in the `get` rule moved to a single unified lookup at the top of `expr_ty`'s
call arm (covers every accessor uniformly). Base *record* refinement stays
field-only (an index step would need a fixed-arity tuple refinement we can't infer
from one position); an index path still narrows the access itself.

**Verified.** New test `path_narrowing_through_index_paths` (nth/first + mixed
field-index warn cases; different-index and consistent-use no-warn); the three
`path_narrowing_*` tests pass; Rust lib 367/367; clippy clean; **`nest check`
still 0 warnings**. This closes the roadmap's "narrowing through non-variable
expressions" item — only a genuinely-unpinnable computed key remains, which is
not a gap.

## 2026-07-07 — Local type inference: the sound (return-only) half

Added a second tier to `infer_sig` — **sound, not complete**, the explicit design
call (the user's framing: "we have to be sound, we don't have to be complete
yet"). Tier 1 (unchanged) is the precise params+return case: a body that's one
direct call to a known-sig callee. Tier 2 (new) is **return-only**: for any other
single-arm body, infer just the return type as `expr_ty` of the body tail with
parameters bound `ANY`. So a multi-step or branchy function's *result* now has a
type and its misuse is caught — `(string-length (wrap 3))` where
`(defn wrap (x) (let (y (+ x 1)) (* y 2)))` returns `number` (wrap 3 = 8, so the
call genuinely errors at runtime).

**Why this is sound where full inference isn't.** The false-positive source in
inference is *parameter* inference across branches: a param used as a number only
inside `(if (number? x) …)` must not be typed number (else `(f "x")` — valid —
warns). Return-only inference never constrains a parameter, so that failure mode
can't arise. And `expr_ty` is a proven over-approximation (soundness oracle) that
already unions branch results, so a branchy body's inferred return is a sound
superset — a disjointness warning on the result is then a genuine error. A
per-thread `InferGuard` re-entry set breaks recursive/mutual call-graph cycles
(return inference runs `expr_ty` → `sig_of` → `infer_sig`); a cycle declines to
infer.

Deliberately **not** done: parameter inference from arbitrary/branchy bodies —
needs guard-aware dominance analysis, deferred until it's false-positive-clean
(ADR-011).

**Verified.** Updated `infers_through_let_alias` (the `wrap` result is now typed;
its *parameter* still isn't) + new `return_only_inference_is_sound` (result-misuse
caught; guarded param not inferred; overlapping union no-warn; recursion
terminates & stays sound). Rust lib 369/369; soundness oracle 2/2; clippy clean;
**`nest check` 0 warnings** across `std/` + `tests/` (~3s, no perf regression) —
the empirical proof that the amplified inference introduces no false positives.

## 2026-07-07 — Gating design + the B0 prerequisite (prototyped, reverted to sound)

Started the design for the roadmap's 🎯 gating item (full gradual consistency in
the checker's decisions), written up in [`type-gating.md`](type-gating.md). Since
reload-soundness already shipped the *workflow* half (re-check on reload, CI
gate), "gating" here is purely the checker's internal decision logic. Grounded two
gaps against the current binary: **A** — undeclared globals carry no tracked type
(declared globals already gate both value-position and call-arg, verified); **B** —
the call-argument check uses `∩`-only `is_disjoint`, missing the *merely-wider*
mismatch the return check already catches with `⊆`.

**Prototyped Gap B (arg-check → gradual `consistent_with`), and it produced a real
false positive — the key result.** `Ty::of_value` gives an int/bool/string literal
the flat tag (`200 : int`, not `{200}`), so `stat(int) ⊆ (or 200 404 500)` fails
even though `200` is in the set. Making literals `dynamic` fixes that FP but then
loses a real catch (a partial-overlap union `(if (> x 0) x "neg")` declared `int`
→ `int|string`, which `⊆` flags but `∩` misses). The two conflict irreconcilably
without **int/bool/string literal-singleton precision (B0)** — the tried-and-
reverted feature. So Gap B is really B0 (track literal singletons) → B1 (arg-check
`⊆`); B1 alone is unsound.

Per the "no false positives, ever" bar, **reverted the whole Gap B prototype** to
the sound `∩`-only arg check (a lone explanatory comment remains at the site).
Verified back to sound: `types::` 369/369, `nest check` 0. The design doc and the
roadmap 🎯 item now record B0 as B1's prerequisite, and the recommended sequencing
(B0 → B1, with Gap A independent and sound on its own). No language behavior
changed this session — the deliverable is the grounded design + the soundness
finding.

## 2026-07-07 — Gating Gap A: undeclared globals get a current-image type

Shipped Gap A from the gating design ([`type-gating.md`](type-gating.md)): an
*undeclared* global defined **exactly once** by `(def g <non-fn-expr>)` now
carries an inferred current-image value type, so its misuse is caught — `(def g 5)`
then `(string-length g)` warns "expects string, got int" (declared globals already
gated; this closes the undeclared case). `check_file` Pass 2.7 counts top-level
defs, and for each exactly-once, non-macro `(def g RHS)` records `expr_ty(RHS)` in
`Ctx::inferred_value_ty` (skipping a function/native result — a defn's arrow is
inferred separately). `expr_ty` (arg check) and `gradual_of` (value/return checks)
consult it *after* the declared value type, always as `dynamic_within` — the `∩`
relation, so it warns only on provable disjointness and a reload that changes the
global is re-derived (ADR-125), never a stale hard proof.

Sound and conservative by construction: a global defined more than once is
ambiguous → stays `dynamic()` (no FP from a redefinition); scoped to same-file
(cross-file needs a heap-wide inferred store — the follow-on); function globals
untouched. **Verified zero corpus false positives** — `nest check` stays at 0
across `std/` + `tests/`.

New test `undeclared_global_current_type_gates_its_use` (misuse warns;
consistent/redefined/function globals quiet); Rust lib 369/369 (+1 = 370); clippy
clean. Gap B stays blocked on B0 (literal-singleton precision) per the design.

## 2026-07-07 — Gating B0: int/bool/string literal-singleton precision

Shipped B0, the prerequisite the gating design ([`type-gating.md`](type-gating.md))
identified for Gap B. `Ty::of_value` now returns the literal *singleton* for an
int (`5 : {5}`) or bool (`true : {true}`) — like it already did for keywords — and
`expr_ty` builds a string literal's `str_lit` where it has the heap. A literal's
static type is now *faithful* (a subtype of its flat tag) rather than an
over-approximation, which:

- **removes the false positive at its root** — `stat({200}) ⊆ (or 200 404 500)`
  now holds, where flat `int ⊄ {200,404,500}` did not — so it also fixes a latent
  FP in the existing return/def checks (a literal-body-vs-literal-set return);
- **unblocks B1** (the arg-check `⊆` upgrade), which was unsound without it;
- makes every diagnostic naming a literal precise: `got 5`, `yields "hello"`,
  `value of type "hello"` (not `got int` / `yields string`).

That last point is the wording churn the earlier attempt balked at — ≈19 checker
test message assertions, updated mechanically (the behavior was already correct;
only the displayed type sharpened). This is `docs/type-int-literals.md`'s once-
deferred "call-site argument literal precision," now done because gating needs it.

Sound by construction: a singleton is the value's *exact* type (a subtype of the
flat tag), so it only sharpens — never over-approximates. Bignum literals stay
flat `int` (`int_lit` is `i64`). **Verified:** `types::` 370/370; clippy clean;
`nest check` 0 across `std/` + `tests/` (the increased precision surfaced no new
warnings — no match-redundancy/dead-clause cascade). B1 is the next step.

## 2026-07-07 — Gating B1: argument check through the full gradual relation (Gap B complete)

With B0's literal singletons in place, shipped B1 — the argument check now runs
the same `gradual_of` / `consistent_with` the return check uses, closing the
return/argument asymmetry. A **precise** argument (a literal singleton, a
`(sig …)`-typed param, integer-closed arithmetic) is checked with `⊆`, so a
*merely-wider* misuse is caught — a `number` sig-param passed where `int` is wanted
now warns "expects int, got number". A **dynamic** argument (a call result, a
redefinable/inferred global) is checked with `∩` (`!is_disjoint`) — identical to
the old behaviour, no new over-warning, reload-safe.

B0 is what makes this sound: a literal is now a faithful singleton, so `(f 200)`
against a `(or 200 404 500)` param does NOT false-positive (`{200} ⊆ {200,404,500}`),
the exact FP that blocked B1 before. Two supporting fixes shipped with it:
`gradual_of` now consults a narrowing on *any* symbol (not just lexical locals —
so a guard-narrowed *free* variable keeps its narrowing in the arg check), and
`consistent_with`'s dynamic branch uses `is_disjoint` (refinement-aware, so a
record/tuple/literal-set conflict on a dynamic argument is caught, matching the
disjointness the flat `∩` missed).

**Gap B complete.** Verified: new test `argument_check_uses_the_full_gradual_relation`
(merely-wider warns; literal-in-set doesn't; dynamic call-result defers); Rust lib
371/371; clippy clean; **`nest check` 0** across `std/` + `tests/` (the `⊆` arg
check surfaced no merely-wider false positive in the corpus). Remaining gating
work: cross-file inferred-global propagation (the Gap A follow-on).

## 2026-07-07 — Cross-file Gap A (and a dynamic-var soundness fix)

Closed the cross-file half of gating Gap A — and it needed none of the pre-pass /
eval-tracking infrastructure I'd first scoped. Realization: cross-file *function*
checking already works because `infer_sig` reads the current closure from the
loaded heap (`obs_global`) — the image is loaded before checking. So an undeclared
*value* global gets the same treatment: `global_value_ty` reads its current heap
value and types it (`Ty::of_value`), consulted last in `expr_ty` / `gradual_of`
(after declared + same-file-inferred), always as `dynamic_within`. `obs_global`
records the dependency, so a change re-checks the reader (ADR-125). No new store,
no order concerns, no eval change.

**Dynamic-var fix (the real soundness catch).** A `defdyn` global's heap value is
only its *default*, but `binding` rebinds it to any type in a dynamic extent — so
typing a use against the default is unsound (`(binding (*d* "s") (string-length
*d*))` is valid but would false-positive against `*d* : {0}`). Excluded dynamic
variables (`value::is_dynamic`) from *both* the new cross-file path and the
already-shipped same-file Pass 2.7, where it was a **latent** hole (the corpus only
escaped it by using its dyn-vars consistently as int).

Shares `infer_sig`'s one narrow, pre-existing FP class (a top-level use that ran at
load before a same-name redefinition) — already accepted for functions, nothing
new. **Gap A is now complete (same-file + cross-file).** Verified: new
`cross_file_undeclared_global_gates_via_loaded_image` (cross-context catch; dynvar
excluded; fn global not gated); Rust lib 372/372; clippy clean; **`nest check` 0**
across `std/` + `tests/` (every cross-module reference, no FP).

## 2026-07-07 — Fix: stack overflow in Tier-2 return inference (deep bodies) + gate cleanup

Running the full `nest test` (which I hadn't re-run since the type-checker work
landed) crashed with a **main-thread stack overflow** in its check pre-flight —
while `nest check` passed. Backtrace: ~1900 nested `expr_ty`/`control_flow_ty`
frames. Cause: **Tier-2 return-only inference** (commit `7732f14`) made
`sig_of` walk a function's whole body via `expr_ty` at *every call site*
(`infer_sig` → `expr_ty(body)`), and `expr_ty`/`control_flow_ty` had **no
recursion-depth bound** — so a function with a pathologically deep (macro-expanded)
body overflowed the type-walk. The `InferGuard` cycle-breaker didn't help: the
depth is *within one body*, not the cross-function chain.

**Fix — two bounds, both sound (a cap yields `None` = "unknown" = defer, which can
only lose a warning, never invent one):**
- `expr_ty` gained a thread-local recursion-depth guard (`MAX_EXPR_TY_DEPTH = 128`)
  — comfortably below the ~1900 overflow, past any real form's nesting, and
  per-thread so it's correct under the parallel checker. This is the actual fix.
- `infer_sig` gained a cross-function depth cap (`MAX_INFER_DEPTH = 8`) on the
  `INFERRING` set — bounds (and de-duplicates the O(N²) re-walk of) a deep
  return-inference call chain; realistic chains are 2–3 deep.

Also cleared two red CI gates found along the way: whole-tree `cargo fmt`
(11 committed files had accumulated drift — the feature-gated `terminal`/`jit`/`gui`
files most of it; purely cosmetic re-wrapping) and one clippy `clone_on_copy` on a
`Value` in a test.

**Verified:** `nest test` now **2605/2605** (was: overflow); `nest check` 0; Rust
lib 372/372; clippy `--all-targets --all-features` clean; `cargo fmt --check`
clean. The tracked known-issues list (`known-issues.md`) remains empty (KI-1–KI-8
fixed, KI-9 transient); this was a regression introduced and fixed within the
session.

## 2026-07-07 — Multi-process RUNTIME GC: Erlang-style 2-generation model (Stages 1a/1b/2)

Replaced ADR-091's deferred *cooperative rolling quiesce* (compact + rewrite every
process's RUNTIME handles — the repo's most race-prone unbuilt design) with what
Erlang's code server actually does: **at most two code generations, no handle
rewriting.** The pivot is enabled by Brood's own invariants — the shared region holds
*only code*, code is *append-only* (hot reload never mutates a live closure), and data
is immutable + per-process — so a generation is a pure add-only epoch that can be
dropped *whole* atomically, sidestepping the cross-process handle-migration that made
the quiesce hard.

Shipped this session (all behavior-preserving — normal runs never age, `current_gen`
stays 0):
- **Stage 1a** (`ad51345`): RUNTIME handles carry a 1-bit `code_gen` tag (GEN bit 32);
  region-aware `canonical()` keeps gen-0/gen-1 same-index handles distinct for
  equality/hashing (LOCAL still masks the full GEN field). `runtime_gen(idx, gen)`
  constructor + `code_gen()` accessor + round-trip tests.
- **Stage 1b** (`b9c4b33`): `RuntimeCode.code` → `gens: [CodeSlabs; 2]` + atomic
  `current_gen`; every accessor / `region_ref!` RUNTIME arm reads `gens[id.code_gen()]`,
  fills go to `cur_code()`.
- **Stage 2** (`da81ac3`): all 12 `promote` mints gen-tag the fresh handle (push helpers
  on `RuntimeCode` for leaf slabs; inline `runtime_gen(idx, cur_gen)` for
  map/pair/closure/env). `Heap::age_runtime()` — a lightweight atomic flip of
  `current_gen` (needs no unique ownership, unlike compaction) that refuses unless the
  target slot is empty (**2-versions-max** — so a new gen's indices can't collide with a
  prior generation's live handles). Compaction guarded to gen 0. New test
  `aging_flips_generation_and_both_gens_execute`: define in gen 0 → age → define in gen 1
  → **both** generations execute (incl. a gen-1 closure calling a gen-0 global), and a
  second age correctly refuses.

Remaining: **Stage 3** cooperative liveness (each process reports old-gen references at
safepoints → union "is the old gen dead?"), **Stage 4** free the dead generation +
soft-wait/purge pins + auto-trigger aging at the RUNTIME safepoint.

**Verified:** Rust lib 375/375; `runtime_collector` 11/11 (incl. `BROOD_GC_STRESS=1
BROOD_GC_VERIFY=1`); clippy `--all-targets --all-features` clean; `cargo fmt --check`
clean; `nest check` 0 warnings; `nest test` 2605/2605.

## 2026-07-08 — Multi-process RUNTIME GC: Stage 3a (the per-process liveness probe)

The next piece of ADR-091's reclamation path: `Heap::runtime_gen_referenced(gen)` — a
read-only probe answering **"is generation `gen` still referenced by any live code, as
seen from this process?"** It's the per-process half of the Stage 3 *union* that will
decide when an aged-out old generation may be freed (Stage 4): a generation is dead only
when every live process (and the shared globals) reports it unreferenced. For a
single-process runtime this heap sees the whole picture, so the answer is exact.

The probe seeds a worklist from the **complete** root set — the shared roots (globals +
declared `(sig …)` type-exprs) *and* this process's private roots (operand/env stack,
dynamic bindings, both LOCAL heap generations, and the live VM arms mid-execution) — then
walks RUNTIME handles transitively, returning `true` the instant it reaches a live handle
in generation `gen`. It reuses the graph-walk shape of `runtime_live_closure_count` but
(a) is generation-scoped (`visited` keys on `(gen, index)` since the two generations share
one index space) and (b) covers the private roots + LOCAL heaps + live arms that the
diagnostic count omits — the completeness a real liveness answer needs. The per-process
**caches** (`vm_cache`/`global_ic`/…) are deliberately *not* scanned: they hold RUNTIME
handles but rebuild lazily, so Stage 4 will clear them when it frees a generation (exactly
as `runtime_collect` already does) rather than treating a cached handle as a live pin;
only the un-clearable mid-execution live arms are scanned.

New test `old_generation_liveness_probe` watches a generation go from referenced to dead:
define `f` (gen 0 referenced; empty gen-1 slot trivially unreferenced) → `age_runtime`
(gen 0 *still* referenced — aging moves no bindings) → define `h` in gen 1 (both gens
live) → redefine `f` into gen 1 + a LOCAL collect (gen 0 now dead — every binding it held
superseded). Behavior-preserving: purely additive read-only introspection, nothing auto-
ages yet.

**Verified:** Rust lib 375/375; `runtime_collector` 12/12 (incl. `BROOD_GC_STRESS=1
BROOD_GC_VERIFY=1`); clippy `-p brood --all-features` clean.

## 2026-07-08 — Multi-process RUNTIME GC: Stage 3b (the cross-process drain union)

The mechanism that turns the per-process Stage-3a probe into a runtime-wide "is this
generation dead?" answer. Shared drain-coordination state on `RuntimeCode` (behind the
`Arc`, so every process of the runtime sees it): `drain_active` / `drain_gen` /
`drain_epoch` + a `drain_acks` map (`pid → the epoch it last reported clean for`). Five
`Heap` methods drive it:

- `begin_gen_drain(old_gen) -> epoch` — arm a drain: bump the **strictly-monotonic** epoch
  and clear the acks, so no ack from a prior drain can count; publish gen/epoch before
  flipping `drain_active` true.
- `report_gen_liveness(pid)` — this process's cooperative report: probe
  `runtime_gen_referenced(drain_gen)`; a clean process acks the current epoch, a process
  still referencing the generation drops its ack (so it pins). A no-op when no drain is
  armed.
- `gen_drained(live_pids) -> bool` — the union: every live pid acked the current epoch. The
  caller supplies the live set (the process layer will read it from the scheduler registry;
  keeping the enumeration out of `core` preserves the layering).
- `end_gen_drain()` / `clear_gen_ack(pid)` — disarm a drain (Stage 4, after freeing); drop a
  dead process's ack (Stage 3c, at process exit).

**Soundness.** A process acks clean only when its probe — which includes the shared globals —
sees no reference to the draining generation. After aging, new code only ever lands in the
*current* generation, so a global can never come to point at the drained one again: a clean
ack therefore stays valid, and a process can only re-acquire a reference by `spawn`ing a
child, which enters the live set un-acked and keeps the union `false` until it too reports
clean. So the union is monotone toward "drained" and never prematurely `true`.

**Inert by default → behavior-preserving.** Nothing arms a drain yet (Stage 4 will), so
`drain_active` stays false and every method is a single relaxed atomic load that returns
early — **zero hot-path change**. Deliberately built and tested as a standalone mechanism;
wiring the report into the live eval safepoint / park path (Stage 3c) is the next,
`BROOD_GC_STRESS`-validated step.

New test `cross_process_drain_union` drives the union deterministically with **two real
heaps sharing one runtime `Arc`** (as `spawn` builds a child) — no scheduler-timing
dependence: main supersedes its `f` into gen 1 (clean), the child captures the gen-0 `f`
handle in a root (pins); the union stays `false` until the child drops the handle and
re-reports, then flips `true`; a fresh `begin_gen_drain` (epoch 2) invalidates the stale
acks; `end_gen_drain` makes the path inert again.

**Verified:** Rust lib 375/375; `runtime_collector` 13/13 (incl. `BROOD_GC_STRESS=1
BROOD_GC_VERIFY=1`); clippy `-p brood --all-features` clean.

## 2026-07-08 — Multi-process RUNTIME GC: Stage 3c (drain union wired into the scheduler)

Wired the Stage-3b drain mechanism into the live scheduler so a genuinely concurrent,
multi-process runtime computes the "is this generation dead?" union. Four small
process-layer functions (`crates/lisp/src/process/scheduler.rs`, re-exported from
`process`):

- `current_pid() -> Option<u64>` — this process's pid *without* minting a ctx (a
  ctx-less process isn't in `REGISTRY` and can't be sharing a runtime under a drain, so
  skipping it is sound). One thread-local borrow.
- `live_pids() -> Vec<u64>` — the `REGISTRY` keys, i.e. the drain union's live set. Complete:
  `spawn` registers a child (before it can run) and its parent, and `receive`/`self` register
  the root, so every process that could pin the draining generation is present.
- `report_drain_liveness(heap)` — the safepoint report: `heap.report_gen_liveness(current_pid)`.
- `old_gen_drained(heap)` — the union answer: `heap.gen_drained(&live_pids())`.

The report is called at **both** engines' eval safepoints — the tree-walker loop
(`eval/mod.rs`) and the VM trampoline (`vm_run_bc`, `eval/compile/mod.rs`) — each gated on a
single `heap.drain_active()` atomic load, so the always-case (no drain) is one relaxed load
and the whole path stays **inert / behavior-preserving** until Stage 4 arms a drain. The
probe is read-only (moves nothing), so it needs no GC/macro gate.

**Soundness across parked processes.** A process reports only while *running*; a parked one
keeps whatever ack it had. That's still sound because of the post-aging no-new-refs
invariant: a process that acked clean can't reacquire a reference to the drained generation
(globals only ever move forward to the current gen), so the probe never needs to scan a
parked continuation; and a process that *does* still pin has no current-epoch ack, so it
blocks the drain — safe, if conservative (Stage 4's soft-wait/purge handles progress).
Per-exit `clear_gen_ack` was deliberately **left unwired**: the ack map is wiped at every
`begin`/`end_gen_drain`, dead pids are dropped from `REGISTRY` (so `old_gen_drained` never
reads them), and pids aren't reused — so per-exit clearing has no correctness role, only
hygiene. Kept the method for Stage 4.

New test `drain_report_wires_through_the_scheduler` (genuinely concurrent, default worker
pool): a spawned worker captures the gen-0 closure and parks holding it; after aging +
superseding every gen-0 global into gen 1, `old_gen_drained` stays **false** (the live
parked worker pins gen 0 with no ack) even though the root reported clean at its own VM
safepoint; once the worker is released — it drops the reference, replies, and exits —
`old_gen_drained` flips **true**. Deterministic despite the concurrency: the outcome hinges
on process *liveness*, not scheduling order (stable across repeated runs).

**Verified:** Rust lib 375/375; `runtime_collector` 14/14 (incl. `BROOD_GC_STRESS=1
BROOD_GC_VERIFY=1`, and `--features jit`); `concurrency_race` green; `jit` 28/28; workspace
builds; clippy `-p brood --all-features` clean; **`nest test` 2605/2605**.

## 2026-07-08 — Multi-process RUNTIME GC: Stage 4 (the free mechanism — ArcSwap generations)

The reclamation payload of ADR-091: a drained generation can now actually be **freed
while the runtime is shared**. Two parts.

**1. Freeable storage — `gens: [ArcSwap<CodeSlabs>; 2]`.** The blocker: the append-only
`boxcar` slabs can't be cleared through `&self`, and `Arc::get_mut` never succeeds with
live processes — so the single-process compactor path can't reclaim a shared runtime. Made
each generation an `arc_swap::ArcSwap<CodeSlabs>`, so a free is an atomic **store of a fresh
empty slab**; the old `Arc` drops when the last reader guard releases it. The cost (chosen
deliberately over an unsafe in-place swap): the reference-returning RUNTIME accessors
(`closure`/`string`/`vector`/`map_node`/`env_frame`/`rope`/`bigint`/`decimal`/`bytes`) now
return a guard-holding **`SlabRef<T>`** (a `Deref<Target=T>` wrapper carrying the ArcSwap
`Guard`) instead of a bare `&T`, so a read in flight during a free keeps the slab alive —
safe by construction. `SlabRef` grows `Deref`/`Debug`/`Display`/`PartialEq`/`AsRef` +
`map` (project to a field), so ~230 call sites compiled unchanged via `Deref`; only the ~20
that needed a real `&T` (fn args, `Ord::cmp`, returning a borrow) took a `&`/`.iter()`/bind.
`promote`/`def` append into the loaded slab's `boxcar` in place, so a store only happens on
a free — never on the hot `def`/read path (reads are one extra `ArcSwap::load`, the tier the
user opted into).

**2. The free — `Heap::free_runtime_gen(old_gen)`** (via `process::free_drained_gen`, gated
on `old_gen_drained`): stores the empty slab, then fixes cross-process cache coherence —
bumps `version` (self-invalidating the version-stamped `global_ic` / call-&-global ICs /
shared JIT caches on every process) **and** a new `free_epoch` (each process lazily clears
its handle-keyed `vm_cache` — not version-stamped — on its next lookup via `sync_free_epoch`,
one relaxed load+compare). That `free_epoch` clear is the subtle correctness point: a freed
slot is reused by aging with **bit-identical `(gen,index)` handles**, so without it a stale
`vm_cache` body could be served for a brand-new closure.

New tests: `free_reclaims_after_cross_process_drain` (two heaps sharing a runtime — the free
is refused while a peer pins gen 0, succeeds once the pin releases, and the slot is then
reusable by `age_runtime`) and `reused_slot_runs_new_code_not_stale_cache` (define→call
[caches the body]→free→age-into-freed-slot→define→call runs the NEW code, not the stale
cached one — the direct guard on the `free_epoch` mechanism).

**Deferred — the auto-arming** (not the mechanism): wiring aging+drain+free into the RUNTIME
safepoint. Surfaced a real design point — **aging migrates no globals**: Erlang reclaims
because a module reload re-exports *all* a module's functions as a unit, but Brood's `def` is
per-global, so a global defined once and never redefined stays in its birth generation and
pins it forever. Auto-arming therefore needs aging to **re-promote the live globals into the
new generation** (copying the small live set, à la a module reload) — else a drain never
completes for stable globals. That + the soft-wait/purge policy for genuine pins is the next
step. Everything here is **behavior-preserving**: nothing auto-ages or auto-frees; a drain is
only armed by a test (or the future trigger). JIT-native code running an old generation is
handled by the drain gate (such a process references the generation → its probe blocks the
free) plus the cache invalidation above.

**Verified:** Rust lib 375/375; `runtime_collector` 16/16 (incl. `BROOD_GC_STRESS=1
BROOD_GC_VERIFY=1` and `--features jit`); `concurrency_race` green; workspace builds; clippy
`-p brood --all-features` clean; `nest check` 0 warnings; **`nest test` 2605/2605**. Perf: the
per-RUNTIME-read `ArcSwap::load` is a new hot-path cost the user accepted for safe-by-
construction reclamation; a release A/B (`make benchmark`) is a follow-up before auto-arming.

## 2026-07-08 — Multi-process RUNTIME GC: Stage 4 auto-arming (live-globals migration + safepoint state machine)

Turned the Stage-4 free *mechanism* into an actual **multi-process collector**, behind the
`BROOD_RT_MULTIGEN` opt-in (default off — the normal path is unchanged).

**Live-globals migration (`migrate_live_globals`).** Aging alone reclaims nothing: it flips
which generation new code lands in but moves no existing binding, so a global defined once
and never redefined stays in its birth generation and pins it forever (Brood's `def` is
per-global, unlike an Erlang module reload that re-exports *all* a module's functions as a
unit). So aging now re-exports the live globals + `declared_sigs` into the fresh generation,
reusing the compaction `flush_rt_*` machinery — generalised to be **gen-selective** (forward
only source-generation nodes; pass through the rest) and to mint **dest-generation-tagged**
handles (`RuntimeForward` carries `old_gen`/`dest_gen`). The reframe: migration is compaction
where forwarding is done by **retaining** the old generation (its handles stay resolvable)
until its holders drain — sidestepping the un-coordinatable cross-process handle rewrite the
abandoned rolling-quiesce needed. The reconcile installs a migrated handle only where the
global still lives in the old generation, so a concurrent redefinition (which lands in the
current generation) wins with no value-equality needed — after aging the old generation is
frozen.

**The state machine (`advance_runtime_multigen`)**, driven at the RUNTIME safepoint on both
engines (added the missing RUNTIME-collect call to the VM trampoline — it was tree-walker-
only): drain in flight → free once the union is clean; idle + other slot empty → age +
migrate + arm the drain (single-flight `begin_aging` CAS); idle + other slot occupied → wait
(2-versions back-pressure). Migration runs **before** arming the drain, so nothing can newly
acquire an old-gen reference once it's live — the invariant behind a new report optimisation:
a process that reports clean for a drain epoch isn't re-walked (bounding it to one liveness
walk per drain). Also fixed the reverse leak: a drain armed while shared that then goes
quiescent is now still advanced/freed (the drain branch runs regardless of ownership).

**A real soundness bug the concurrency test caught.** A generation flip on one process could
interleave with an in-flight `promote` on another: `promote` reserves a slot in the current
generation then fills it re-reading `cur_code()`, so a flip in between filled the *wrong*
generation's slab (panic / cross-generation-split closure — a hard crash under the
concurrent multigen test). Fixed with a `promote_lock` `RwLock`: promotion holds it read
(concurrent lock-free `boxcar` appends are fine), `age_runtime` holds it write, so no promote
spans a flip. Uncontended on the default single-generation path (one bare read-lock per
`def`/`spawn`).

Tests: three new deterministic mechanism tests in `runtime_collector.rs` (migration lets a
generation with a stable global still free; migration preserves a post-aging redefinition;
the full age→migrate→drain→free cycle repeats and stays bounded across cycles), plus a new
`runtime_multigen.rs` end-to-end binary (six real workers churn `f` under `BROOD_RT_MULTIGEN`
while the root hot-reloads it 400×; the collector ages + migrates mid-flight and never
miscompiles — every `(f 0)` stays 0, and it ages ≥1 generation). New observables:
`runtime_free_count` / `runtime_aged_count`.

**Deferred — the purge policy.** A process permanently looping in old code pins its
generation (Erlang's purge condition); with a permanent pin, churn can't reclaim past the 2×
ceiling. Today's policy is the safe option: don't age a third time (accept 2×) until the pin
clears. Erlang-style soft/hard purge (signal or kill processes in old code) is a separate
future decision — hence the opt-in default.

**Verified:** Rust lib 375/375; `runtime_collector` 19/19 (incl. `BROOD_GC_STRESS=1
BROOD_GC_VERIFY=1` and `--features jit`); `runtime_multigen` green ×5 + under `--features
jit`; `concurrency_race` green; clippy `--all-features` clean; `nest check` 0 warnings;
**`nest test` 2605/2605**.

**Perf note (for the deferred A/B):** the VM trampoline now evaluates `rt_gc_due()` per
call-frame (an `ArcSwap` load + a `boxcar` count) where before it was tree-walker-only, and
every `promote` takes an uncontended `promote_lock` read. Both are on paths the multi-process
collector needs; neither is believed hot, but a release A/B (`make benchmark`) before flipping
`BROOD_RT_MULTIGEN` on by default should confirm the per-call `rt_gc_due` cost is negligible
(and, if not, gate it behind a cheaper promote-bumped counter).

### Stage 5 — soft purge: parked-process drain inspection (`check_process_code`-style)

Closed the parked-can't-ack hole in the multi-process RUNTIME collector. A generation frees
only once every live process reports clean at an eval safepoint — but a process **parked** in
`receive` never reaches a safepoint, so a drain armed *after* it parked could never collect
its ack, and an idle server parked on current-gen code would block every later drain forever.

Fixed by *external inspection* (`process::report_parked_liveness`, driven by
`old_gen_drained`): a paused process's continuation is relocatable heap data (ADR-100) —
its live values live on its own `Heap`'s `roots`/`env_roots`/`live_vm_arms`, and the
scheduler holds that heap in the mailbox `waiter` slot — so the drain coordinator walks each
parked process's own quiescent heap and lets it ack if clean of the draining generation. No
wakeup, no kill: a parked-clean process stops stalling the drain; one genuinely paused *in*
old code stays dirty (correct). This is the Erlang `check_process_code` model exactly, and
strictly a *soft* purge — it removes only *false* pins, never a live one.

Verified: `crates/lisp/tests/runtime_drain.rs` (own binary, so the global process `REGISTRY`
holds only its own processes — a leaked/parked worker mustn't pollute another drain test's
`live_pids()` union). A worker parks on gen-1 code while a drain of gen 0 is armed; the drain
completes only because the parked worker is inspected and acked — confirmed to **deadlock
(5 s timeout) without the inspection**, pass (~0.5 s) with it. Full gauntlet green: lib 375,
runtime_collector 19, runtime_multigen (×2), concurrency_race, plus the Brood suite (2605/0).

**Aside (pre-existing, noted not fixed):** `(spawn worker)` with a *named global* 0-arg fn
exits `:normal` **without running its body** — the body must be wrapped in a thunk
(`(spawn (fn () (worker)))`), which every existing test already does. Surfaced while writing
the Stage-5 test; unrelated to this GC work, but worth a look as a possible `spawn` bug.

Still deferred (see ADR-091): the harder purge rungs for a genuinely *pinned* generation —
a `recur-latest` re-dispatch convention (its own small ADR — new language surface) and a
hard purge (kill + supervised restart). `BROOD_RT_MULTIGEN` stays default-off pending those
+ the perf A/B.

### Stage 5 soundness fix — re-home a `def`'d value out of the draining generation

A review of the multi-process collector (ADR-091 stages 3–5) surfaced a real, if
latent (`BROOD_RT_MULTIGEN` is off by default), use-after-free. `promote` is a no-op
on an already-RUNTIME value, so `(def k v)` with `v` resident in the **draining**
generation stored that stale handle straight into the shared globals table — an
un-walked drain root — *after* migration had moved the live globals off it. That
re-pins a generation a process already reported clean for; if that process then
exits, the drain union can go all-clean and `free_runtime_gen` empties a generation a
global still points at → the next lookup of `k` dereferences a freed slab (panic /
miscompile). The same shape also let migration's reconcile clobber a concurrent
`(def k old-gen-value)` (a lost def).

Fix: `Heap::rehome_to_current` — a `def`/`sig` of a value in a non-current RUNTIME
generation now deep-copies it into the current generation (the same `flush_rt_value`
machinery migration uses), under `promote_lock` so an aging flip can't relocate the
target mid-copy. Wired into the global `env_define` and `set_declared_sig` paths — the
two shared roots the drain scans. This keeps the invariant "no shared root points at
the draining generation" intact, so the drain gate stays sound, and a concurrent def
is no longer mistaken for a stale binding. No-op on the default single-generation path
(the fast-path gen check returns immediately).

Verified: `runtime_collector.rs::a_def_of_an_old_gen_value_is_rehomed_off_the_freed_generation`
— a straggler binds a gen-0 handle after migration, then gen 0 is freed; with the fix
`(g)` returns 42, without it the test panics with the exact `runtime closure handle`
use-after-free the review predicted. Full suite green (2610/0), collector 20, lib 375.
Also polished the bytes-literal bad-escape message to match the string path.

The review also confirmed the storage/guard machinery, promote⇄age locking,
parked-process inspection (lock ordering + quiescence), and the 2-generation cap are
sound; and that the footgun fixes (spawn/if/def/escape) have no correctness issues.

### Perf A/B on the multi-process GC — a per-call RUNTIME-safepoint regression, fixed by sampling

Ran the clean eval A/B (baseline f85e5c9 vs the ADR-091 stage-3–5 tree) on an idle
machine. The call-heavy micro-benchmarks regressed on the default VM engine — `fib(25)`
24.2ms → 30.8ms (~+27%), with `apply_driven`/`reduce_range` also up — while
`cons_build`/`sum_tail` were flat. A controlled same-tree experiment (gate the check out,
rebuild, re-bench) pinned it: `fib(25)` dropped to 22.2ms with the RUNTIME safepoint
disabled, so the cost was `rt_gc_due()` itself.

Cause: stage 3–5 added a RUNTIME-region safepoint to the *VM* trampoline (it was
tree-walker-only before — a real leak fix: VM programs now compact the shared code
region). But `rt_gc_due()` loads the shared-code `ArcSwap` guard + counts its closures,
and it ran **every frame**. RUNTIME only grows on `def` (a `promote`), which never happens
inside a hot compute loop, so the probe's *cost*, not its result, was the tax.

Fix: sample it in `vm_run_bc` — run the probe on frame 1 of every `vm_run_bc` (once per
top-level form / resume, so a script of many short `def`s still collects promptly) and
every 256th frame of a long run (so a hot loop pays ~1/256 of the cost). This changes only
the check *frequency*, not which conditions trigger a collect, so the drain state machine
is untouched: `runtime_multigen` still ages/migrates/drains under load (verified 3×, plus
under `BROOD_GC_STRESS`), collector 20/20, drain green. `fib` returns to ≈baseline.

Note: the benches carry ~±15% run-to-run variance, so the smaller `apply_driven`/
`reduce_range` deltas are within noise and not separately actionable; `fib` was the one
clean, reproducible signal, and it's resolved. BROOD_RT_MULTIGEN stays **off** by default
(this was a default-path fix, independent of the multigen feature flag).

### RUNTIME safepoint, take 2 — a dirty-bit gate (replaces the frame sampler); plus a located HOF regression

A thermally-controlled **interleaved** A/B (baseline `f85e5c9` vs main, alternating rounds
so both see the same CPU thermal state — the earlier back-to-back full run was contaminated
by a uniform ~+25% throttle on the *second* run, visible as an across-the-board tree-walker
slowdown on an unchanged engine) gave a much cleaner read. Two findings:

1. **The frame-sampler was replaced by a dirty-bit gate.** Instead of sampling `rt_gc_due`
   every 256 frames + frame 1 (which still cost one `ArcSwap` load per `vm_apply`, taxing
   HOF loops), the safepoint now gates on `heap.rt_dirty()` — a relaxed `AtomicBool` on
   `RuntimeCode` set at the *sole* RUNTIME closure-mint point (`promote_closure`, i.e. every
   `def`/`spawn`/hot-reload) and cleared once the probe runs — plus `drain_active()`. A
   def-free hot loop (`fib`, `reduce`, `apply`) trips neither and skips the `ArcSwap`+count
   entirely; a mint re-arms it so a collect is at most one frame late. Min-of-3 interleaved:
   `fib/25` +1.0%, `sum_tail/100000` −0.2%, `cons_build/100000` +0.9% — all flat (the
   per-frame `rt_gc_due` tax is gone). Collector unaffected (only the probe *frequency*
   changes): `runtime_multigen` ages/drains under load + `BROOD_GC_STRESS`, collector 20,
   drain green, full suite 2610/0.

2. **A separate, real HOF regression is located but NOT yet fixed.** `apply_driven/100000`
   (+16%, consistent every round) and `reduce_range/1000000` (+17%) regress vs `f85e5c9`,
   and this is *not* `rt_gc_due` (a controlled "probe fully disabled" build still showed it)
   and *not* thermal (interleaved). Root cause: Stage 4 changed `cur_code()` from `&CodeSlabs`
   to an `ArcSwap` `Guard` load and `closure()` from `&Closure` to a guard-holding `SlabRef`
   (necessary so a drained generation can be **freed while the runtime is shared**). So every
   closure deref now does an `ArcSwap::load`. `fib` is immune — its self-call resolves once
   through the call-site inline cache — but `apply`/`reduce`/`map`/`fold` resolve the callee
   *per element* via `apply_value`, which has no IC, so they pay one `ArcSwap` load per call
   (~15ns × 1e6 ≈ the measured +16%). This is a default-path cost imposed by an off-by-default
   feature. Proposed fix (deferred — a safety-critical hot-path change deserving its own cycle
   with `BROOD_GC_VERIFY`/stress): epoch-keyed per-process memoization of `rt_slab_ref`,
   reusing the existing `free_epoch`/`seen_free_epoch`/`vm_cache` pattern. On the default path
   `gens[0]`'s `Arc` never changes (age/free only fire under multigen; single-process compaction
   mutates in place via `Arc::get_mut`), so the cache would load once and never invalidate.

### The HOF regression, fixed — epoch-keyed memoization of the RUNTIME slab read

Closed the +16% `apply`/`reduce`/`map`/`fold` regression from the previous entry (the
`ArcSwap`-per-closure-deref that Stage 4 introduced so a generation can be freed while the
runtime is shared). `Heap::rt_slab_ref` now caches each generation's `Arc<CodeSlabs>`
per-process and skips the `ArcSwap` load, revalidating against a new
`RuntimeCode::gens_epoch` bumped at the only two sites that replace a `gens[_]` Arc
(single-process compaction's commit + `free_runtime_gen`). A projected `&CodeSlabs` is
handed out with `&self`'s lifetime (the cached `Arc` lives in the heap), so the common
deref is a relaxed epoch load + a pointer read — no atomic `ArcSwap` protocol.

Soundness rests on a gate: the fast path runs **only when the multi-process collector is
disabled** (`!rt_multigen_enabled()`, the default). Verified from the code that every
`gens` store then happens single-threaded — compaction is `Arc::get_mut`-gated to the sole
process, and the only direct-free caller off the flag (`runtime_collector`) is
single-threaded, while every *multi-threaded* free path (`runtime_multigen`) sets
`BROOD_RT_MULTIGEN`. So no generation is ever freed concurrently with a fast-path deref,
and a synchronous deref can't observe a mid-use `Arc` swap. When multigen *is* armed,
`rt_slab_ref` falls back to the original `ArcSwap` `Guard` (which defers a concurrent
free's drop) — that branch is byte-identical to the pre-change code.

Result (interleaved A/B, baseline `f85e5c9` vs fix; round-1 same-thermal pair):
`reduce_range/1e6` −3.8%, `apply_driven/1e5` +0.5% — both back to baseline from +17%/+16%;
`fib`/`sum_tail` flat. Validation: default full suite 2610/0; `runtime_collector`
(single-threaded age/migrate/**free** — exercises exactly the epoch invalidation) 20/20
under `BROOD_GC_STRESS`+`BROOD_GC_VERIFY`; `runtime_multigen` (multigen on → Guard path)
green under stress; lib 375/0 under stress+verify. (The *full* suite under
`BROOD_RT_MULTIGEN=1` is too slow to complete in-budget — active whole-generation
collection across 2610 tests — but the Guard path is unchanged, so the default-path suite
plus the multigen unit tests cover both branches.)

## 2026-07-09 — Sequence API: shrink the lazy surface to `l*` + `->>`, drop transducers/`eduction`

Simplified the public lazy/fusion surface to the smallest coherent set and hid
the plumbing. The design question (lazy-by-default? auto-fusion? explicit?) was
settled first: **explicit lazy** wins under Brood's constraints — lazy-by-default
breaks the entrenched "iterate for side effects" idiom (`(map require-one …)`,
`(map run-test …)`), and immutability forbids a memoizing lazy value (no mutable
thunk cell), so a lazy default would silently re-compute or drop effects. Auto-
fusion (rewrite `(map f (filter p xs))` at compile time) was rejected too: it
erodes hot-reload late-binding at the fused call sites and is more magic than the
BEAM's explicit `Enum`/`Stream` split, which is the model we already mirror.

**What changed (all in `std/prelude.blsp`):**
- **Public lazy surface = `lmap` / `lfilter` / `lkeep` / `lremove` only**,
  composed with the standard `->>`. Each returns a lazy `%seqview`; chaining
  fuses the stages onto one view so a `->>` pipeline folds/reduces in a single
  pass with no intermediate lists.
- **Removed from the public surface:** `eduction`, `transduce`, `transduce--step`,
  `reduced`, `reduced?`, `deref-reduced`, and the `xmapcat`/`xtake-while`
  transducers (and their `reduced`-based early-termination protocol) — deleted
  outright, since nothing but the now-gone public `transduce`/`eduction` reached
  them.
- **Hidden, not removed:** the four transducer constructors the `l*` combinators
  actually use are renamed `%xmap` / `%xfilter` / `%xkeep` / `%xremove` (the
  established `%`-prefix internal convention). `comp` stays public (general
  function composition).

**Measured (n = 2e6, release+jit):** fused `(->> (range n) (lfilter even?)
(lmap inc) (reduce + 0))` ≈ 0.5 s vs eager `(reduce + 0 (map inc (filter even?
(range n))))` ≈ 1.5 s — **~3×**, and it holds for both cheap and expensive
per-element work (the win is avoiding the two large intermediate lists, not
per-element cost). Crossover: a *single* lazy stage is slightly slower than eager
(view overhead, no intermediate to eliminate), so fusion only pays for **2+
stages over large data**.

**Stdlib retrofit — deliberately NOT done.** Surveyed `std/` for fusion
candidates: exactly one 2-stage `map`/`filter` chain exists (`diff.blsp:110`,
over a tiny op-list); everything else is single-stage or cold tooling. None is
both hot and large, so retrofitting would be inert churn that trades clear eager
code for lazy+realise with no measurable win — and would violate the "optimize
only when it improves perf broadly and builds a real capability" bar. The fusion
capability already exists; it pays off in *user* code over large data, which is
where it belongs.

**Correctness / invariants verified.** A view is **heap-local**: `send` *refuses*
to ship one (`cannot send a lazy seq-view in a message; realise it first`) rather
than silently shipping a heap-referencing value — the immutability/network guard
holds. Added explicit coverage in `tests/sequence_test.blsp`: `->>`-fused
pipelines, a view rejected by `send`, a realised view round-tripping through a
worker as a deep-copied list, N workers each fusing a view over a shared global
concurrently, and a view honouring hot-reload late-binding of a global its fn
calls (redefine `view-scale` before realise → fold sees the new def). Green:
`nest check` 0 warnings, full suite 2614/2614, `sequence_test` 76/76 under
`BROOD_GC_STRESS`, `BROOD_GC_VERIFY`, and `BROOD_RT_MULTIGEN=1`+stress. Docs
updated (`language.md`, `brood-for-claude.md`, `compute-frontier.md`,
`llm-native.md`, `writing-brood-skill.md`).

## 2026-07-09 — Multigen RUNTIME GC: diagnosed the suite hang (pre-existing), fix attempts reverted

Investigated why `BROOD_RT_MULTIGEN=1` can't run the suite (the devlog's "too slow to
complete in-budget"). Built a faithful repro — 300 green processes, each promoting ~200
distinct accumulating RUNTIME `def`s, crossing the RT-GC floor so real collection cycles
fire — and measured **12–20× slowdown** vs default *and* an **intermittent hang** (a run
either finishes in ~2–7 s or wedges past 45 s).

**Root cause (measured, not guessed).** With the RT-GC floor raised so migration never
fires, multigen ≈ default (389 ms vs 329 ms) — so the whole cost is the collection cycle.
Instrumenting the cycle showed migration itself is cheap (one ~26 ms copy of ~3.8 k live
globals) and the generation **never frees** (`freed=0`): a long-lived process (here the root
running `collect`, and every worker still executing its own load-time `gen0` code) genuinely
*pins* the draining generation, so the drain can't complete for the whole run. While a drain
is armed the threshold is pinned to `count`, so **every process re-enters the safepoint
collector every frame** and runs `free_drained_gen` → `old_gen_drained` → `report_parked_liveness`,
a whole-`REGISTRY` scan that locks every mailbox — O(P²·safepoints) of lock traffic — while
each *pinning* process also re-walks its roots and takes the `drain_acks` write lock every
safepoint. The result is a contention livelock that intermittently looks like a hang. Confirmed
**pre-existing**: a worktree build at `HEAD` (no changes) hangs *more* often (2/2) than any
patched build.

**Fix attempts — all reverted.** Tried, in `heap.rs`/`scheduler.rs`: (1) single-flight the
parked-liveness scan (one scanner per frame); (2) a pid-ceiling "drain cohort" so processes
born after the drain armed (which provably can't hold an `old_gen` handle) don't block the
union under churn; (3) eliminate the no-op `drain_acks` write on the pinning path. These cut
the working-case time ~3.6× (≈7 s → ≈1.9 s) but did **not** remove the intermittent hang, and
worse, the pid-ceiling change **broke two committed drain tests** (`cross_process_drain_union`,
`free_reclaims_after_cross_process_drain`): those construct a pinning child with a fixed low
pid, so the ceiling wrongly excluded a process that genuinely pins the generation — the
ceiling assumes scheduler-monotonic pids and is unsound against the tested contract. The
remaining livelock is deeper than the drain bookkeeping (a scheduler-level progress issue the
multigen safepoint overhead exposes) and needs `ptrace`/`rr` + a deterministic repro to pin —
both unavailable here (yama blocks `ptrace` in this sandbox). So the whole change set was
reverted to the committed state (all 22 multigen/collector/drain Rust tests green, 3×).

**Where this leaves multigen.** Correct and adequate for its real use case — bounded-live-set
hot reload where old versions *supersede* (single-process and multi-process steady-state both
measured at parity with default). It stays **opt-in / off by default**. The blocker for
default-on is this pinned-generation contention livelock, not throughput. A real fix wants:
(a) don't force the O(P) union scan every safepoint when a drain legitimately can't complete
(back off, but without the lost-wakeup a naive `2*count` backoff caused — the free must still
be re-attempted); (b) throttle a *pinning* process's per-safepoint self-report; (c) a
scheduler-lock-order audit of the Stage-3c parked-liveness inspection vs message delivery /
unpark; (d) proper concurrency tooling to verify. Deferred as its own focused piece of work.

## 2026-07-09 — Multigen RUNTIME GC: fixed the drain livelock/hang (private self-report walk)

Root-caused and fixed the intermittent hang under `BROOD_RT_MULTIGEN` (the one the
previous entry reverted three failed attempts at). Characterization: a hung process runs
at ~1100 % CPU (12 threads in `R`, none blocked) — a **CPU-spin livelock**, not a deadlock,
and **bimodal** (~2 s or >45 s). The repro (300 processes each promoting ~200 accumulating
`def`s) hangs; a `HEAD` build hangs *worse* (confirming it's pre-existing).

**Root cause.** The drain self-report `report_gen_liveness` calls `runtime_gen_referenced`
to decide if this process still pins the draining generation — and that probe **seeds every
shared global + `(sig …)` into its walk**. On the accumulate-everything workload that's the
whole 60 k-entry global table, walked **on every safepoint by every still-pinning process**
(a long-lived process legitimately executing old-gen code never acks, so it re-walks forever).
O(P × globals) per round, and it takes the shared `globals` read lock each time. That work
saturates all cores and starves the very processes whose progress would end the drain — the
livelock. It only bites the many-globals shape, which is why realistic hot-reload (bounded
live set) never hit it.

**Fix (one file, +43/−11).** A **private-only** probe `runtime_gen_referenced_private` for
the self-report: walk only this process's own roots / local heap / live VM arms, **not** the
shared globals/sigs. Sound by the collector's own documented invariant — the drain arms only
after `migrate_live_globals` moved every value off the draining generation, and post-aging no
shared root can ever point at it again, so seeding the globals provably contributes nothing;
the only way a process can genuinely pin the generation is a handle it captured *privately*
before the migration, which the private walk still catches. The full `runtime_gen_referenced`
(globals included) is unchanged for its other role as the general liveness probe the tests
assert against. The walk drops from O(globals) to O(this process's own state) — no throttle,
no semantic change to the union.

**Result.** The repro goes from intermittent-hang / bimodal 2–45 s to a **consistent
~1.3 s, 15/15 hang-free** (default, no collection, is ~0.25 s). All 22 multigen/collector/
drain Rust tests green (3×); full default suite 2614/2614. Three earlier dead-ends are
recorded in the prior entry — the mistake there was treating the symptom (throttle the scan /
the acks write) instead of the cause (the O(globals) self-report walk).

**Still open (separate, non-hang).** The whole in-language suite under `BROOD_RT_MULTIGEN`
is hang-free now but still too slow to finish in-budget (>400 s vs ~11 s default): the
per-file migration + the O(live-process) `report_parked_liveness` union scan (still run every
safepoint during a drain) + the post-migration cache-invalidation re-resolve, multiplied
across 2614 tests. That's a throughput optimization (a sound scan-throttle + reducing the
cache-clear blast radius), not a correctness blocker. Multigen stays opt-in; the hang — the
thing that made it *unusable* — is gone.

## 2026-07-09 — Multigen RUNTIME GC: closed the throughput gap (two-phase self-report walk) — suite at parity

The prior entry left multigen hang-free but ~35× too slow to finish the in-language suite
(>400 s vs ~12 s). This closes it: **the whole in-language suite now runs under
`BROOD_RT_MULTIGEN` in ~12–13 s — parity with the default 12.3 s, 2614/2614, 3× stable.**

**Method — measure, don't guess.** Two plausible culprits were ruled out *by A/B*, not
reasoning: (1) the per-closure `rt_slab_ref` `ArcSwap` guard under multigen (added a fast-path
escape hatch, A/B'd: fast ≈ guarded, **zero** gain — reverted, kept the conservative guard);
(2) multigen merely being *enabled* (enabled-but-cycle-disabled ran at full default speed —
so the cost is entirely the age/migrate/drain **cycle**, not the flag). Diagnostic counters
then showed the drain bookkeeping was already cheap (15 advances, 1 migrate, 0 frees) but
`walk_ms=2210` of a 2298 ms run — **the drain self-report walk was ~96% of the cost.**

**Root cause.** `runtime_gen_referenced_impl` (the private self-report probe) seeds the
**entire local heap** into its work-list *before* the transitive walk can early-exit. A drain
lingers whenever a long-lived process pins the old generation (Erlang's local-call code-pinning
limitation), so that process re-reports every safepoint — and each report re-seeded its whole,
still-growing local heap even though its *live VM arm* (the actual pin) would answer the query
in O(1). O(heap) × tens of thousands of reports = the 1.7 s.

**Fix — two-phase walk (semantics-preserving).** Split the probe: **Phase 1** seeds only the
cheap roots (private stack/env/dynamics + live VM arms), walks to fixpoint, early-exits; **Phase
2** seeds the full local heap and continues **only if Phase 1 came up clean**. A pinning process
(overwhelmingly: one *running* old-gen code) short-circuits in Phase 1 without ever paying the
O(heap) seed. Same seed set, same transitive rule, `visited` carried across phases — identical
answer. `rounds` 2170 → 421 ms; `nestlike` unchanged-fast. Extracted the transitive loop into
`walk_reaches_gen`.

**Plus three residual-cost trims** (each measured to matter once the walk was cheap): a
per-process **clean-ack `Cell` cache** (`acked_drain_epoch`) so an already-clean process skips
the `drain_acks` read lock every frame; **dropping the dirty-path `acks.remove` write lock**
(it was a no-op under clean-stays-clean, but serialized P writers every safepoint); a
**free-attempt scan throttle** (`RT_DRAIN_SCAN_STRIDE = 64`) so the O(live-process) registry
scan runs 1/64 safepoints instead of every one; and **gating the RUNTIME-collect safepoint on
`rt_dirty` alone** (not `drain_active`) in both the VM (`vm_run_bc`) and tree-walker (`eval`),
so a lingering drain no longer forces the `cur_code()` `ArcSwap` load on every frame — the
collect/free rides mint frames, while the O(1) self-report still runs every frame.

**Verification.** Two-phase Phase-2 correctness exercised directly: 300 RUNTIME closures
captured in *local data* (not on the running arm) across 60 def-churn rounds under
`BROOD_GC_VERIFY=1 BROOD_GC_STRESS=1 BROOD_RT_MULTIGEN=1` — correct result, no verifier/tripwire
fire (Phase 2 detects the pins; the still-referenced gen is not freed). 726 Rust tests green,
23 multigen/drain/migration Rust tests green, reload tests green under multigen, full 2614 suite
green under both default and multigen. The A/B `rt_slab_ref` fast-path and all diagnostic
counters were removed before commit. Multigen remains opt-in, but is now *viable by default* —
the throughput blocker is gone.

## 2026-07-09 — JIT is now a default cargo feature (on for everything)

Following the multigen-unconditional simplification, made the **tier-1 JIT (ADR-101) a
default cargo feature** so the whole toolchain is uniform: bare `cargo build`, `cargo test`,
`make test`, rust-analyzer, and the shipped `brood`/`nest` binaries all get it — no more
`--features jit` dance. Previously JIT was default-on only for the *product* binaries
(`make run`/`release`/`install`, `WITH_JIT ?= 1`) but **off for `make test` and bare cargo
builds**, so the suite didn't exercise what ships.

Wiring: `crates/lisp` `default = ["dev-tools", "treesit", "jit"]`; `crates/cli`
`default = ["brood/dev-tools", "brood/jit"]` (it pins `default-features = false` on the brood
dep, so it must name `brood/jit` explicitly). `nest`/`brood-lsp` depend on brood with default
features, so they inherit it. The opt-out is unchanged and still `--no-default-features`: the
lean `nest release` bundle strips it and re-adds via `brood/jit` only when the host supports
Cranelift (`./configure --without-jit` → `WITH_JIT=0` keeps it stripped). The per-crate `jit`
features are kept for those `--no-default-features` builds.

Cost accepted (per the deliberate choice): every clean `cargo build`/`cargo check`/
rust-analyzer now compiles the four Cranelift crates (`cranelift-codegen` is a whole codegen
backend). In exchange the tested config == the shipped config, and there's one engine story.

Verified: plain `cargo build --release --bin brood` (no feature flag) produces a JIT-active
binary (`BROOD_JIT_DUMP_IR=1` emits `[jit-ir]` lines); full `make test` now **763 tests, 763
passed, 1 skipped** — up from 726 because the `jit` unit/e2e and `differential` binaries are
built by default now. Docs updated (CLAUDE.md perf-build note, Makefile `WITH_JIT` comment).

## 2026-07-09 — Remove dead complexity: the inert PoisonBits tripwire + two stdlib dups

Cleared the remaining survey items from the earlier kernel/stdlib sweep. No behaviour
change; full `make test` 764/764 (debug_assertions on, so the epoch tripwire is exercised
on every deref), GC stress+verify clean on the churn repros.

**PoisonBits removed (`heap.rs`, ~235 lines).** The debug-only `PoisonBits` use-after-GC
tripwire has had **no writer** since the in-place mark-sweep's `sweep` was deleted — the
copying collector relocates survivors and drops the dead wholesale, never freeing a slot in
place — so every `PoisonBits::is(...)` answered `false` and every `debug_assert!(!poisoned)`
was a no-op. It was fully superseded by the generational-handle epoch tripwire
(`check_epoch_aged`, ADR-054). Deleted the struct/impl/field/inits, the two flush-time
`.clear()` blocks, the `env_is_poisoned` / `debug_walk_env_chain` / `env_chain_debug`
(`BROOD_ENV_DEBUG`) diagnostics, and their three `eval/mod.rs` call sites. Simplified the
`local_gc_check!` macro (dropped the now-unused `$poison`/`$label` params) and the
`region_ref!` LOCAL arm. **The active `check_epoch_aged` tripwire is byte-for-byte
unchanged** — the diff only removes the inert poison lines around it (verified: no
`check_epoch_aged` call added or removed).

**Two stdlib duplicates removed.** `datetime/dt--fmod` (floor-mod) → the prelude's Euclidean
`mod`: identical for the one call site's positive divisor 7. `stats/frequencies` was a slower
duplicate of the prelude `frequencies` (not part of the stats module's documented API, and
`mode` already calls it bare → prelude); deleted it and its now-redundant test block.

**Assessed and deliberately left** (a little duplication beats the wrong coupling, and
"stabilise" says don't silently change public behaviour): `file`↔`path` path helpers *look*
duplicated but diverge — `file/path-extension` uses the prelude `path-basename` (keeps
trailing slashes) while `path/extension` uses `path/basename` (strips them), so merging would
change public behaviour. The five open-coded hex encoders (`encoding`/`hash`/`uuid`/`json`/
`url`) are each ~4 self-contained lines; consolidating would couple foundational crypto
modules onto `encoding` and risk digest correctness for negligible gain.

## 2026-07-09 — Multigen RUNTIME GC is now unconditional (ADR-091) — flag + dual paths deleted

Closing the record on the multigen thread. With multigen proven at parity the prior three
entries (hang fix → drain-livelock fix → two-phase throughput → suite parity), the two
shared-runtime strategies were collapsed into one and the opt-in machinery deleted
(commit `09fd96a`). This supersedes the "Multigen remains opt-in, but is now *viable by
default*" line closing the throughput entry above: it is no longer opt-in — it is the only
shared-runtime path.

Not a behaviour change for the default path: a shared runtime already couldn't reclaim
RUNTIME code without multigen (it just leaked via exponential back-off); now it always
reclaims via age/migrate/drain/free. Single-process compaction is unchanged and still
handles the uniquely-owned case — the two are complementary by ownership, not alternatives.

**Removed.** The `BROOD_RT_MULTIGEN` flag, its `OnceLock`, and the env read; all four gate
sites (`maybe_runtime_collect` drain-priority + `None`-branch advance + threshold) are now
unconditional. Also the `rt_slab_ref` fast-path slab cache (`rt_gen_cache` `UnsafeCell` +
`seen_gens_epoch`) and the `gens_epoch` that existed solely to invalidate it — always the
guarded `ArcSwap` load now (measured ~0 cost on the shipped JIT path — fib(33) 22 ms either
way — and ~5% only on the no-JIT fallback, in exchange for deleting an `UnsafeCell` and its
use-after-GC aliasing invariants). Net −67 lines in `heap.rs` (−129/+41 across the commit).

**Verified** (in the commit): full in-language suite 2615/2615 on the now-default path;
full `make test` (Rust + Brood, no-JIT) green; 23 multigen/drain/migration/collector Rust
tests green; Phase-2 detection correct under `GC_VERIFY` (300 closures captured in local
data across 60 churn rounds, no verifier fire). `CLAUDE.md`, `docs/roadmap.md`, and ADR-091
were updated at the time; this entry backfills the devlog step that the commit skipped.

## 2026-07-09 — brood-life feedback triage: shipped the accepted cluster + ADR-130

Worked the whole accepted set from the brood-life language-feedback triage (roadmap ACTION
item) in one pass. Each item was checked against the tree first — which mattered, because
three "headline" asks turned out already done and would have been wasted work.

**Shipped (all pure Brood except one thin I/O primitive):**
- **`clamp` + `as->`** (`std/prelude.blsp`). `(clamp x lo hi)` = `(max lo (min hi x))`;
  `as->` binds the thread value to a name and expands to one *sequential* `let` (Brood's
  `let` re-binds a repeated name, so `(as-> 5 $ (+ $ 1) (* $ 2))` → `(let ($ 5 $ (+ $ 1)
  $ (* $ 2)) $)`). Tests in `suite_test.blsp`.
- **`{:keys […]}` / `:or` map destructuring.** The pattern matcher is a Brood pattern→code
  compiler in the prelude, so this is a Brood change, not a VM one: a `match-map-pattern?`
  (`map?`) arm in `match-compile`/`pattern-vars`, plus `match-compile-map` emitting
  `(if (map? t) <lets> fail)` where each `:keys` symbol binds to `(get t :sym)` (or the
  `:or` default via a presence check, evaluated only when absent). Because `let`/`fn`/`match`
  all delegate to this one compiler, map destructuring works at every binding site for free.
  The namespace resolver's `collect_all_syms` and the checker's binder tracking already
  walked `Map` patterns, so `nest check` stayed clean with no checker change. General
  `{:key subpattern}` nesting and `:as` deferred (ADR-011). Tests across
  `pattern_matching_test.blsp`.
- **`nest mcp eval` multi-form.** `read-string`'s trailing-form drop was *already* fixed
  (it errors on trailing content; `read-all`/`read-first`/`eval-string` exist) — but
  `mcp-eval-tool` still used `read-first`, so a pasted batch ran only its first form. Now
  `eval-string` (eval-all, returns the last value). New `mcp_test.blsp`.
- **`spit-append`** (`builtins/io.rs`) — a thin `O_APPEND` string-append primitive (the
  ADR-006 mechanism exception; string sibling of the existing `append-bytes`).
  `std/file.blsp`'s `append-file` was a non-atomic slurp+concat+spit read-modify-write; it's
  now a `spit-append` alias — atomic and O(1) per write. `file_test.blsp` gains a 40-process
  concurrent-append race that asserts all 40 distinct lines survive.

**Design settled, not built — ADR-130.** `defrecord` (the review's top pick) contradicted the
standing "model data with plain maps" stub, so it needed a decision, not a slipped-in macro.
ADR-130 rules it: `defrecord` is **pure prelude sugar over closed maps** — a positional
constructor + one accessor per field + an optional per-field `sig` lowering to the existing
`(record …)` type (ADR-115); zero new core, records *are* maps, accessors turn field-name
typos into checker-caught undefined-function errors. Accepted as direction; implementation is
a follow-up.

**Doc fixes (both verified against code).** `spec.md §11` listed four shipped features
(dynvars, map literals, modules, tracing GC) as "not yet specified" — rewritten to the real
gaps. `value-repr.md` said the `Value` enum is 16 bytes; the kernel hard-asserts **24** (the
`Pid { node, id }` variant needs two payload words) — corrected throughout, drift noted.

**Already-done, ruled out of the work (verified, no code):** JIT-default-on (`b9c3a20`), the
parallel-allocation lock fix (`67c2ec2`), and `transients.md` (rewritten 2026-06-15).

**Verified.** `nest check` clean (zero warnings across `std/` + `tests/`) — the map-pattern
binders need no checker change. Targeted suites green: `suite_test` 64/64, `pattern_matching_test`
112/112, `mcp_test` 5/5, `file_test` 18/18 (incl. the concurrent-append race). Full `make test`
green.

## 2026-07-10 — `defrecord` implemented (ADR-130) — pure prelude sugar over maps, zero new core

Built the `defrecord` slice ADR-130 scoped. It came in at exactly zero new core, as the ADR
predicted: no `Value`, no `Tag`, no special form, no builtin.

**The macro** (`std/prelude.blsp`). `(defrecord point (x y))` expands to a positional
constructor `(defn point (x y) {:x x :y y})` plus one accessor per field `(defn point-x (p)
(get p :x))`. The constructor body is built with `(zipmap field-keywords field-syms)` — which
produces a `{:x x :y y}` map *literal* whose values are the field symbols; because an unquoted
map literal evaluates its values (verified), the constructor evaluates the args at call time.
A field may be `(name type)`; when **every** field is typed, `(sig …)` forms are emitted too —
`(sig point (int int -> (record :x int :y int)))` for the constructor and `(sig point-x
((record :x int) -> int))` per accessor — lowering to the record types that shipped in ADR-115.

**Why the accessors are the point:** `(point-witdh p)` is an undefined-function reference the
checker flags; `(get p :witdh)` is forever silent. So the sugar buys typo-safety at zero cost.
Records *are* maps, so `assoc`/`merge`/`=`/pattern-match/`send` all keep working and there is
no `point?` predicate (structural, not nominal — ADR-130).

**Also:** removed `defrecord` from the `eval/mod.rs` unknown-form hint (it now covers only
`deftype`/`definterface`/`reify`, and points them at `defrecord`); added `defrecord` to the
`SPECIAL_FORMS` highlight list (`kw::DEFRECORD`), so grammar/LSP/treesit colour it from the one
source.

**Two findings while building** (both recorded, neither a blocker):
- **Cross-file needs no scanner change.** The worry was that the native def-head scanner
  (`scan-source-extract`) doesn't macroexpand, so a record's constructor/accessor names might
  not resolve from another file. Tested with a 2-file `nest` project: the accessors resolve
  *and* a typo'd accessor is flagged cross-file — the checker resolves record names via loaded
  globals, not just the raw scan. So the def-head lists were left untouched.
- **The static checker doesn't arg-type-check any `sig` at call sites.** `(point "a" 4)` with
  the typed constructor is *not* flagged — but neither is a hand-written `(sig f (int -> int))`
  + `(f "a")`, so this is the checker's existing behaviour, not a `defrecord` gap. Per-field
  type enforcement therefore lands via return-type flow and `BROOD_CONTRACTS=1` runtime
  contracts (verified: under contracts, `(point "a" 4)` is rejected). The typo-safety win is
  fully static. Noted in ADR-130's status.

**Verified.** `tests/record_test.blsp` (9 tests incl. a cross-process round-trip) green;
`(special-forms)` includes `defrecord`; the `deftype` stub now points at it; `nest check`
clean; full `make test` green.

## 2026-07-10 — Checker now flags a wrong-type `sig` argument at the call site

Closed the gap the `defrecord` entry above flagged: the static checker *did* argument-type-check
call sites (the ADR-110 gating-"B1" arg check has been in `walk.rs` for a while), but the check
was **dead inside a `defmodule`** — so no real program ever hit it. Pass 2.5 keyed user `(sig …)`
declarations under the **bare** name, while a call head resolves to the module-**qualified**
`ns/name`, so the two never matched and the sig silently never seeded. Now `(point "a" 4)` with a
typed constructor — and any `(sig f (int -> int))` + `(f "hello")` — is flagged at the call site.

**Three fixes** in `types/check.rs` pass 2.5:
- **Qualify user sig names.** New `qualify_decl_name` runs each parsed `(sig …)` name through
  `macros::qualify_name(file_ns, name)` (guarded by `is_file_global`, so a bare root-namespace sig
  still keys bare) — the same qualification `defn` heads already get. `register_declared_sig`
  applies it across all four sig shapes (`parse_sig_decl` / `_with_vars` / `_overload` / `_value`).
- **Recover macro-emitted sigs.** `defrecord`'s `(sig …)` forms live *inside* macroexpansion, but
  pass 2.5 reads un-expanded text. `collect_register_sig_forms` walks the **expanded** forms for
  `(%register-sig 'name 'type)` (the expansion `sig` lowers to) and rebuilds a `(sig name type)`
  for the checker — so a record constructor's per-field arg types are enforced statically too.

**Enabling the arg check surfaced three pre-existing false positives, all fixed properly** (not
by muting the check):
- **`(list T)` includes `nil`.** `Ty::list_of` masks to the `SEQ_BITS` (Pair|Vector) by lattice
  design — the empty list is `nil`. `relax_param_for_arg` (walk.rs) re-adds `Nil` to a list-typed
  param *for the membership test only* (the message still reports the declared type), so
  `(sum-list nil)` against `((list int) -> int)` no longer misfires.
- **A record param drops its optional fields for the arg test.** The record-shape subtype relation is
  conservative — a literal `{name}` isn't a subtype of `{name, age?}` even though the value satisfies
  it (the optional `age` is just absent) — so requiring the optional field's *declaration* false-flags
  a valid arg. `relax_param_for_arg` keeps only the *required* fields, so a missing/wrong-typed
  required field is still caught (a guard-refined record still flows a real conflict into a call — the
  `path_narrowing_refines_base_record_type_into_calls` capability), while an omitted optional passes.
- **A `& rest` binder is `list<elem>`, not `elem`.** `check_fn_seeded` was seeding the rest param as
  the sig's rest *element* type, so `(defn f (& xs) (reduce + 0 xs))` typed `xs` as `int` and then
  flagged `(reduce … xs)`. New `params_form_has_rest` + `Ty::list_of` seeding fixes it.

**New suppression category `:type-mismatch`** for `(check-allow …)` — a `sig`-declared return/argument
a negative test *deliberately* violates (proving the `sig!` runtime contract throws) opts out, the
same escape hatch `:non-tail-recursion` / `:unreachable-clause` already provide. `contract_test.blsp`'s
`c-bad-ret` (which returns a string under an `(int -> int)` sig on purpose) uses it — that warning is
now *correct*, since the qualification fix made the sig actually attach.

**Verified.** New Rust tests (`sig_call_site_wrong_literal_arg_is_flagged`,
`check_allow_type_mismatch_suppresses_call_and_return_lints`) green; `nest check` clean across
`std/` + `tests/` (isolated worktree, 0 warnings); full `make test` green.

## 2026-07-10 — Type-checker gating: reconciled docs to the shipped state + reload-aware invariant

Audited "what's next for the type system" and found the checker's gating work is
further along than the roadmap said. **Gap A (cross-file inferred-global
propagation)** — an *undeclared* global typed from its current-image heap value
where it's used in another file — was already implemented (`global_value_ty` in
`types/check/guards.rs`, consulted by `gradual_of`) and unit-tested
(`cross_file_undeclared_global_gates_via_loaded_image`), but the roadmap still
carried the whole "wiring `dynamic()` / full gradual consistency" item as 🎯.
Verified end-to-end in a scratch project: a cross-file `(def max-port 8080)`
misused as `(string-length max-port)` warns `expects string, got 8080
(hello/max-port)` and `nest check` exits 1. The only follow-on genuinely left is
the reload *re-check trigger* beyond `nest run --watch` (REPL/LSP push), which is
a reload-workflow concern in `type-soundness-reload.md`, not checker decision
logic — that surface is now feature-complete.

**Reload-aware invariant, written down.** The reload-soundness mechanism
(ADR-123/124/125/126) shipped a while back, but `CLAUDE.md` and `docs/types.md`
compatibility contract #5 still carried the older "checking never rejects a
runnable program" phrasing. Revised both to the precise form: *the checker never
gates the live image, and never warns on a use valid for the image's current
state* — a `def`/reload always wins, the checker re-derives on every reload, and
the one hard reject is batch/CI only (`nest check`'s nonzero exit). Contract #4
gained a clause noting a global may carry a tracked current-image type but always
as `dynamic_within(T)` (the `∩` relation), never a precise `stat(T)`. Updated
`type-gating.md`'s status header (design → shipped) and its "invariant this
revises" section (TODO → done).

**Drive-by:** `nest check` on the repo had drifted to two warnings —
`tests/serve_test.blsp` bound two spawned-client pids (`a`/`b`) it never used
(from the buffer-markers commit). Renamed to `_a`/`_b` (the exempt-from-lint
idiom, per `chaos_test.blsp`'s `_w (spawn …)`); back to zero warnings. Doc-only
+ test-rename change; `types::` tests and the serve suite (7/7) green.

## 2026-07-10 — Dead-clause lint broadens to precise surface `let`-locals (ADR-131)

Shipped the roadmap's last ⬜ type-checker item: the dead-clause lint now flags a
guard that narrows a **precise surface `let`-local** to the empty type, not just a
sig-typed param. `(let (port 8080) (cond (string? port) …))` now warns
`unreachable clause: port is 8080, which can never be string — this branch is dead
code`.

**Design.** A second eligibility set, `Ctx::dead_clause_locals`, alongside
`sig_params`; the scan (`newly_dead_sig_param` → `newly_dead_binding`) walks both.
A `let`-local joins it only when its RHS is **precise** (`gradual_of.dynamic ==
false` — a literal / integer-closed expr; a call-result or redefinable global is
`dynamic` → excluded, keeping the verdict reload-safe) and its name is **surface**
(non-gensym; factored `is_gensym_sym`, also de-duped out of the unused-let lint).
That binding-level gate *is* the surface-vs-generated scoping the roadmap flagged
as the prerequisite — a macro tests its own gensym temps, never the user's named
local, so no guard-site position check is needed (the sig-param lint never used
one either; my first cut added a position gate and it wrongly suppressed
`cond`-generated `if`s, which lack positions — removed). Sound because a local is
immutable, so an over-approximated-but-precise type narrowed to `never` genuinely
proves the branch dead.

**Verified.** Two new checker tests (catch + the three exclusions:
compatible-narrowing, `dynamic` call-result RHS, gensym; plus a shadow-drops-
eligibility case). `types::` 235/235. `nest check` across `std/` + `tests/` stays
at **zero warnings** (no corpus false positive), and a real project catches the
`(let (port 8080) …)` case end-to-end.

**Doc reconciliation (audit of "what shipped vs the docs").** Fanned out two
audits. The type-system roadmap section had no further drift (the earlier Gap A
fix was the only one). The broader sweep found three stale markers, now fixed:
lazy sequences were ⬜ though the **fusing lazy seq-views** shipped (ADR-111) —
split to 🟡 (seq-views done, unbounded `iterate` still ⬜); the **JIT tier-1**
parent was ⬜ though the core shipped and is a default cargo feature — now 🟡 (only
native-IC + float-spec sub-stages remain); and `spec.md §11` still listed
rest-param `sig` notation (ADR-127), lazy seq-views (ADR-111), and `defrecord`
(ADR-130) as unshipped — trimmed to just unbounded `iterate` + the `#{…}` set
literal.

## 2026-07-10 — JIT native-IC increment 2: re-confirmed NO-GO; pivot to leaf inlining

Picked up "JIT tier-1: native-IC" from the roadmap. Profiled first (the project's
own rule: don't ship perf work that doesn't move the benchmark). **Key reframe:**
fib is the wrong benchmark for the call-dispatch path — its self-calls compile to
direct native recursion and bypass the fast-frame path entirely (dispatch ~0%, 86%
in the native arm). The right shape is a **delegator** — a small helper called
non-tail in a hot loop: `(defn add1 (n) (+ n 1)) (defn work (n acc) (if (< n 1) acc
(work (- n 1) (+ acc (add1 n)))))`. On that, `perf` self-time is `jit_run_fast_link`
37%, `brood_rt_fast_frame` 15%, `brood_rt_push` 7.5%, table/base re-fetches ~3% —
**~63% of runtime is the call-dispatch plumbing** increment 2 targets, and toggling
increment 1 off (`BROOD_NO_JIT_ICALL=1`) is 5.33 s vs 4.05 s on, so the path is real.

Designed increment 2 (inline the frame setup + `call_indirect` into IR; env-rooting
split by env-word bit 62 — Stable/GLOBAL done in IR, movable bails to
`brood_rt_fast_frame`) and landed sub-increment **2a** — the `roots: Vec<Value>` →
`Roots { buf, len }` split that makes the frame length IR-writable (behavior-neutral,
`make test` 769 green under GC stress). **Then found in git history that increment 2
was already built end-to-end, verified correct, and reverted for regressing ~5%**
(`3e196ab`/`269b77a`/`f0dfd15`, 2026-06-19; fib(38) in-IR 1.25 s vs FFI 1.19 s) — the
`icall_enabled()` comment even says "don't retry this lever." The measured cause is
decisive: **the dispatch cost is the irreducible frame setup + `call_indirect`, not
the FFI boundary** — `brood_rt_fast_frame`'s FFI call is cheap, LLVM compiles the
frame work (`resize`/nil-fill) better than hand-emitted Cranelift, and the in-IR path
*adds* per-call eligibility checks. Two adjacent attempts also regressed
(`jit-optimizing-tier.md` §6a FFI-collapse, §6c early self-inliner). So the ~63% I
measured is genuinely there, but it can't be *cheapened* — only *removed*.

**Reverted everything** (2a commit dropped, tree clean at `e69b1c1`). **Pivot: the
call-heavy win is inlining the callee, not cheapening the call** — Technique B Phase 2
(leaf callee inlining), which splices a small non-recursive callee into the caller so
the call/frame/dispatch vanish. Genuinely unbuilt; reuses the self-inliner's splice +
dual-body/per-engine-frame-sizing infra (the thing that made self-inlining
net-positive globally), hot-reload safety free via the `compile_epoch` guard. Warm-
start plan (measure-first: a throwaway Phase-0 prototype + full `make benchmark` A/B
gates the whole lever, since every adjacent attempt regressed) in
`~/.claude-personal/plans/peaceful-tinkering-valley.md`; roadmap updated (Stage 3
inc-2 marked NO-GO, leaf inlining + the bigger heap-walk-tiering gap logged as
to-try). To be done as a fresh focused effort. **Net code change this session: zero**
— the value is ruling out a known-failed lever before wasting the build, and the
correctly-aimed pivot.

## 2026-07-10 — House-cleaning sweep: seven bugs fixed, one scheduler liveness bug found (deferred)

A broad "clean house" pass. Baseline was already pristine — full suite green
(769/769), zero build/clippy warnings, `nest check` clean, no TODO/FIXME, no flaky
tests (distribution 3×, GC-stress concurrency all clean). So the sweep went semantic,
fanning out reader/kernel/std/process/editor bug-hunts. Fixed:

- **Markers stranded across undo/redo** (`std/editor/buffer.blsp`). `buffer--snapshot`/
  `buffer--restore` carried only `{:rope :point :mark}`, so undo reverted the text but
  left `:markers` frozen at their post-edit positions (only re-clamped to length on
  read). Fix: snapshot/restore `:markers` too — like `point`/`mark`, restored wholesale
  rather than re-derived — preserving the marker-less-buffer invariant. +3 regression
  tests.
- **Range index-math i64 overflow → host panic** (`core/heap.rs`, `builtins/sequences.rs`).
  `range_len` overflowed its i64 span (`(count (range i64::MAX))` panicked); five range
  walkers stepped an unguarded `i += step` that overflowed on the final step
  (`reduce`/`join`/`range_to_vec`/hash). Fix: compute `range_len` in i128 + saturate;
  `checked_add`→break in every walker. +2 regression blocks in `arithmetic_edge_test`.
- **`int->char` silent truncation** (`builtins/sequences.rs`). `n as u32` truncated an
  out-of-range codepoint, aliasing a valid char (`(int->char (+ 65 2^32))` → "A")
  instead of erroring. Fix: `u32::try_from` guard before `from_u32`.
- **`path/normalize` kept `..` above absolute root** (`std/path.blsp`). `(normalize "/..")`
  → `/..` (should be `/`); the reducer didn't know it was absolute. Fix: thread the
  `absolute` flag and drop a `..` that hits an empty stack (relative paths still keep
  leading `..`). +2 regression tests.
- **`queue/pq` ties popped LIFO** (`std/queue.blsp`). `<=` inserted equal-priority items
  ahead of existing ones. Fix: strict `<` → stable/FIFO ties; documented + regression test.
- **Two wrong docstrings** — `stats/variance` (claimed it raises on a 1-element seq; it
  correctly returns 0.0) and `path/relative-to` (claimed "returns p unchanged with no
  common prefix"; it correctly emits `../..`). Docstrings corrected to match behavior.
- **Stale `CLAUDE.md`** — `eval/compile.rs` (now the `eval/compile/{mod,ir,jit_lower}.rs`
  split), the removed `std/proc/hatch` (now `proc/{gen,supervisor}`), and the
  "net/supervisor are external packages" narrative (re-bundled in-tree by ADR-097).

**Found but deferred — a real scheduler liveness bug: `(exit pid :kill)` cannot kill a
process blocked in a native-nested `receive`** (i.e. a `receive` reached through `try`/
`%isolate`/a HOF, so it blocks the worker on `mailbox.cv` instead of capturing). `exit`
only wakes via `wake_parked` (the green-waiter slot, empty here) and never
`cv.notify_one()`; and the block path (`wait_for_message`/`receive_match`) has no
`kill_pending` check. Confirmed with a clean repro: a plain parked `receive` dies on
`:kill`, but `(try (receive …) (catch …))` with an empty mailbox ignores the kill
indefinitely. The `exit`/`scan_mailbox` comments claiming a "`receive_match` loop-top
`kill_pending` check" are stale — no such check exists. **Not patched here** because a
correct fix is cross-subsystem surgery in the runtime's most delicate code: there is no
`Control::Kill` signal (the `Control` enum is `Suspend`-only), so unwinding a
native-nested receive untrappably through `try` needs a new control signal handled at
`vm_run_bc` + the tree-walker + `run_one`, plus waking the cv from `exit` — and it needs
concurrency stress verification. Deserves its own focused, reviewed effort, not a rushed
patch buried in a cleanup batch. Repro saved.

Also surfaced, left as **language-design calls** (not bugs to patch unilaterally):
number-shaped tokens `++`/`--`/`...`/`1+`/`2+3` raise "malformed number" instead of
reading as symbols (the deliberate loud-failure policy over-captures conventional
identifiers, and diverges from the tooling `scan_atom_kind` classifier); symbols/keywords
built from arbitrary strings via `(symbol "…")` don't round-trip through `pr-str`/`read`
(no `|…|` escaping); the JSON parser accepts `+5`/`01` (over-lenient vs strict JSON).

## 2026-07-10 — House-cleaning, part 2: the deferred bug + all three design calls fixed

Followed up the earlier sweep by fixing everything it had flagged-but-deferred.

- **Scheduler: `(exit …)` can now kill a process blocked in a native-nested `receive`**
  (behind a `try`/`%isolate`/HOF, the §7.4 worker-block carve-out) — **ADR-132**. Previously such a
  process ignored the exit forever — `exit` only woke the green-waiter slot and the block
  path had no kill check. Added a `Control::Kill` signal (the `Control` enum was
  `Suspend`-only): `exit` now `cv.notify_one()`s a cv-blocked receiver (mirroring
  `deliver`), `wait_for_message` bails when `kill_pending` is already set (closes the
  lost-wakeup race — `request_kill` publishes the flag before taking the state lock),
  and `receive_match` unwinds with `Control::Kill` on wake. It rides the error channel
  untrappably (`try`/`%try`/`%isolate` re-raise `is_control`), and `vm_run_bc` turns it
  into `VmOutcome::Killed`, retiring the process with the reason still in `state.kill`
  (both hard `:kill` and soft exit; the reason distinguishes them). Verified: try-nested
  + HOF-nested, hard + soft, normal-message-still-delivered, `after`-timeout-still-fires;
  8/8 exit tests, 12× race-checked, GC-stress clean. Corrected the stale `exit`/
  `scan_mailbox` comments that claimed a "`receive_match` loop-top kill check." Two
  refinements surfaced running the full suite: (1) only the **top-level** body driver
  (`capture`) may turn a `Control::Kill` into `VmOutcome::Killed` — a nested `vm_apply`
  run (a `map`/`try` callback) re-raises so the kill keeps unwinding, else it hit the
  "nested vm_apply does no kill capture" `unreachable!`; (2) the block-path kill check is
  gated on `in_capture_run()` so it fires only inside a scheduler-run green process (where
  a driver exists to convert it) — on the root / file-runner thread (e.g. the `nest test`
  collector's native-nested receive under `%isolate`) a `Control::Kill` would just leak as
  an empty-message error, and that thread isn't a killable process, so it keeps the old
  block-and-ignore behaviour.
- **Reader: `++`/`--`/`...`/`1+`/`2+3` read as symbols, not "malformed number" errors.**
  `numeric_shape` now requires genuine numeric intent — a digit present AND every `+`/`-`
  in a valid sign position (leading, or right after `e`/`E`) — so conventional identifiers
  fall through to symbol while real typos (`1e`, `1.2.3`, `1e+`) still fail loudly. This
  also reconciles the reader with the `scan_atom_kind` tooling classifier (they now agree).
- **Printer/reader: `|…|` bar-quoted symbols + `:|…|` keywords** (**ADR-133**) — the round-trip form for
  a symbol/keyword whose name isn't a clean token (`(symbol "a b")`, `(keyword "")`,
  `(symbol "123")`, empty, `.`). `pr-str` emits bars only when needed (never for clean
  names); `str`/`print` stay raw. A shared `scan_bar_body` on the scanner backs the reader,
  the tooling CST, and `scan-tokens`, so all three agree on where a bar token runs.
  `(read (pr-str x)) == x` now holds for every symbol/keyword. Documented in `language.md`.
- **JSON: strict number grammar** (RFC 8259). `string->number` accepted a leading `+` and
  leading zeros; `json--strict-number?` now validates the munged token against
  `[-] (0|[1-9]digit*) (.digit+)? ([eE][+-]?digit+)?`, rejecting `+5`/`01`/`1.`/`.5`/`1e`
  while keeping `0`/`-5`/`0.5`/`1e10`/`1E+3` and array/object contexts.

All four shipped with regression tests (exit, reader-malformed round-trip, json strict,
plus the existing tooling suites). `nest check` zero warnings, clippy clean. Recorded as
**ADR-132** (`Control::Kill`) and **ADR-133** (`|…|` bar-quoting).

A follow-up verification pass (adversarial review + fresh-territory hunt) turned up two
more crash-on-malformed-input bugs, both fixed:
- **`bundle.rs` `mounted()`** did an unchecked `u64` add/sub on the attacker-controlled
  archive-len from a bundle footer — a crafted `alen` near `u64::MAX` overflowed (panic
  under debug-assertions; a wrapped seek + ~16 EiB `vec!` capacity-overflow panic in
  release) instead of the documented "degrade to not-a-bundle." Mirrored the sibling
  `footer()`'s `checked_add`-then-guard. +unit test.
- **`terminal.rs` `parse_hex_color`** sliced `&h[i..i+2]` on a 6-*byte* string, panicking
  when a multi-byte char (`#a€bc`) put a non-char-boundary at the slice — reachable from a
  user face map. Switched to byte indexing like the 3-nibble case (non-ASCII → clean None).
  +unit test. (The dist `wire.rs`/`handshake.rs` framing + `bundle.rs` `footer()`/
  `parse_archive` were audited and are sound: length caps, `checked_add`, constant-time
  MAC, depth/peer caps.)

## 2026-07-10 — CI back to green + text-mode UTF-8 read-boundary carry (byte-faithful I/O closed)

Two fixes from a stability sweep of the language.

- **Red `main` → green: right-sized the Ackermann JIT torture case.** CI was failing
  on `unbox_torture_test › multi-arg non-tail recursion (Ackermann)` — a **timeout**,
  not a correctness bug (full suite passes locally). `ack(3,10)` is ~45M calls peaking
  at recursion depth ~8189, well past the JIT's 1400-frame depth cliff, so it grinds on
  the slow boxed-drain deopt path: ~4 s locally but >120 s on a slow shared CI runner,
  hard-killed at the per-test cap. The prior `:isolated` marker (`ab8d916`) treated
  *contention*, which isn't the cost here. `ack(3,10)` adds **no** code-path coverage
  over `ack(3,8)` — same multi-arg non-tail recursion, same depth-cliff deopt — so it
  was dropped in favour of `ack(3,6)` (in-worker, depth 509 < cliff) + `ack(3,8)`
  (crosses the cliff → boxed drain): identical coverage, ~16× less wall-clock. 125 ms,
  12/12. CI green (`daad66e` → run passed).

- **Byte-faithful `proc`/`net` I/O — closed the text-mode residual.** The `bytes`
  value + binary mode (shipped 2026-06-28, `b84e223`/`733f00c`) already handed up raw
  bytes for byte-framed protocols. The remaining gap was **text mode**: each reader
  decoded its 64 KiB chunk with a standalone `from_utf8_lossy`, so a *valid* multi-byte
  UTF-8 character (emoji/CJK) split across a read boundary was mangled into U+FFFD even
  for perfectly valid UTF-8. Fixed with one shared helper, `process::chunk_payload`
  (+`chunk_flush`): in text mode it splits `carry ++ chunk` at the longest valid-UTF-8
  prefix, delivers that, and carries only an *incomplete trailing* sequence (≤3 bytes;
  a lone continuation / over-long lead is a hard error → flushed immediately, so no
  unbounded-growth DoS) to the next read; a genuinely invalid mid-run stays lossy as
  before. Binary mode flushes any text-mode carry ahead of the bytes, so a mid-stream
  `set-binary` never drops or reorders bytes. Routed **all five** readers through it:
  plaintext socket, TLS client (`tls_exchange`), TLS server (`tls_server_loop`), and
  proc stdout + stderr. The now-stale "Latin-1 carrier / no bytes value" module docs in
  `net.rs`/`proc.rs` were corrected. Tests: 6 deterministic unit tests on
  `chunk_payload` (reassembly, one-byte-at-a-time dribble, invalid-byte lossy, binary
  faithfulness, flush) + an end-to-end proc test (`cat` echoes a >64 KiB run of 3-byte
  chars — 65536 % 3 ≠ 0 *guarantees* a boundary-straddling char, so it fails without the
  carry and passes with it). Green: bytes 23/23, proc 10/10, http 20/20, tcp 5/5,
  slurp_bytes 9/9, scram_bytes 8/8; clippy clean. Roadmap item marked done.

## 2026-07-10 — Multigen RUNTIME GC: fix the ~300× spawn-scaling regression

Making the 2-generation RUNTIME collector unconditional (ADR-091) surfaced a
cliff on spawn-heavy workloads: the `spawn` benchmark (fan out N processes,
each `fib(15)` → `send`) went **140 ms → 45 s** once the RUNTIME region crosses
`BROOD_RT_GC_FLOOR` (default 4096). Suspicious signature: **worse at N=10 000
(45 s) than N=50 000 (1 s)**, and CPU pegged at **~113 %** (one core), 49 s of
user time — i.e. a *single-threaded* quadratic, not a lock storm.

Chased it with sampled instrumentation (`perf` was blind — the hot frames are
Cranelift JIT native code with no symbols). The drain self-report
(`report_gen_liveness` → `runtime_gen_referenced_private`) has two phases: Phase 1
walks the process's cheap private roots + live VM arms; **Phase 2 walks the
process's *entire* LOCAL heap** to catch a RUNTIME closure handle embedded in
immutable data. A process pinned by such a handle is *dirty*, never acks, and so
re-runs the walk on every safepoint. Instrumentation nailed it: `pid=1` (the
root), `epoch=1` (a *single* drain epoch — not many), `dirty=true`, re-walking a
heap that grew 27 k → 65 k cells **80 000+ times** — O(heap × safepoints),
quadratic. (10 k cycles the drain to completion and keeps re-walking; 50 k
saturates the 2-versions-max backpressure and freezes the drain, so it's *faster*.)

Fix — **throttle the Phase-2 re-walk** (`heap.rs`): once the private probe finds a
process dirty via Phase 2 for a drain epoch, it records `(p2_dirty_epoch,
p2_dirty_tick)` and reports its cached stale-dirty verdict, re-validating with the
full O(heap) walk only every `P2_REVALIDATE_STRIDE` (64) safepoints. Sound: a
stale-dirty verdict only ever *delays* completion (the process stays pinned) —
never fabricates a clean ack, so a referenced generation is never freed early; the
authoritative free path (`gen_drained`, un-throttled) still guards the actual
reclaim. **Phase 1 stays un-throttled**, so a process that turns clean by dropping
a root reports it at its very next safepoint (the drain-completion unit tests rely
on that promptness). Refactored the fused probe into `seed_phase1_and_walk` /
`seed_phase2_and_walk` so the private path can throttle Phase 2 while the
authoritative `runtime_gen_referenced_impl` runs both unconditionally.

Companion hardening (defends a *different* O(processes²) path — a many-parked
server under a drain): the drain-completion gate in `old_gen_drained` is now O(1)
— `drain_acked + parked_count < live` bails before the parked-process inspector
runs, since running workers that still pin the gen keep it un-completable anyway;
and `report_parked_liveness` early-returns on a new global **parked counter**
(`PARKED` in `mailbox.rs`, maintained by a single `set_status` choke point on every
`ST_WAITING`-boundary crossing, squared up by `deregister`'s `clear_parked` for a
process killed while parked). Also dropped a now-redundant per-safepoint
`drain_acks` **read lock** in `report_gen_liveness` (the ack `Cell` already
subsumes it).

Result: **N=10 000 spawn 45 s → 1.9 s**, correct checksums, flat scaling
(10 k 1.9 s / 20 k 1.5 s / 50 k 1.0 s). Full suite (777 Rust + 2699 Brood) green;
`runtime_collector` (20) + `runtime_drain` (1) green under plain release **and**
the debug-assertion epoch tripwire; spawn checksums correct under
`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`. clippy + rustfmt clean.

## 2026-07-11 — Closure creation caches its parsed template (ping-pong ~7.5%)

With the scheduler direct-handoff (236b71f) having removed the futex/context-switch
cost, green↔green ping-pong (500k round-trips, 2.13 s vs Elixir 0.57 s) is now
*entirely* userspace. Profiling (`perf`, clean release + debuginfo) found the
standout waste: every `receive` rebuilds its matcher closure `(fn (msg) …)`, and the
VM's `Inst::MakeClosure` re-ran `eval::make_closure` → `list_to_vec` + `parse_params`
+ pass-through analysis over the **immutable** `fn_rest` AST on *every* creation
(~1 M times in the loop). `parse_params` was 1.7 %, and re-walking the RUNTIME
`fn_rest` cons list drove `code_gen_pinned` (the per-deref Arc pin) to 3.0 %.

Fix — **memoise the parsed closure template per `MakeClosure` site.** New
`ClosureTemplate { arms, doc }` (value.rs) is the parse-once result: a pure function
of the AST, only the captured env varies per instance. `make_closure` split into
`parse_closure_template` (parse + pass-through analysis) and `build_closure`
(clone arms + attach env). `make_closure_cached` (the two VM `MakeClosure` exec
sites) keys a per-process cache (`Heap::closure_tpl_cache`) by the `fn_rest`
`PairId`. Invalidation *mirrors `code_gen_pinned` exactly* — a RUNTIME `gen_version`
bump (the only event that relocates the AST handles the arms hold) clears the map —
so a hit is provably current-generation and correctness reduces to code_gen_pinned's
established contract. **Only a RUNTIME `fn_rest` is cached** (a LOCAL slot can be
reused by a minor GC without bumping gen_version); a LOCAL/PRELUDE key falls back to
a plain parse. `SymbolHasher` (a `PairId` hashes as one u64) keeps the per-creation
lookup off SipHash; pass-through is precomputed into the template so
`alloc_closure_pre` skips the re-analysis.

Result: `parse_params` / `list_to_vec` / `compute_passthrough` gone from the hot
profile, `code_gen_pinned` 3.0 % → 0.43 %. **Green↔green ping-pong 2.13 s → 1.97 s
(~7.5 %)**, ~3.7× → ~3.45× vs Elixir. fib / nqueens / spawn flat (no
closure-in-loop). Full suite (777 Rust + Brood) green incl. the dev-profile GC
tripwire; new `vm_closures_test` cases (a *fresh* capturing + multi-arity closure
built per iteration, cross-process fan-out) pass under both engines and under
`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`. The residual per-instance arm-`Vec`
clone/drop (~4.8 %) needs `Closure.arms: Arc<[ClosureArm]>` to remove (it touches the
GC's in-place arm-handle rewrite — deferred). rustfmt clean.

Aside (pre-existing, unrelated — confirmed on the pre-change HEAD binary): the
**tree-walker** (`BROOD_VM=0`) hangs on any cross-process `spawn`+`receive` inside a
`:isolated` test block. Its legacy engine predates the VM's capture-suspend
green-process receive (ADR-100 §8.4); `make test` runs the default VM engine, so
this is never surfaced there. Noted, not fixed (out of scope — the tree-walker is
being phased out per ADR-076).

## 2026-07-11 — The buffer-process protocol grows up (myedit's actor endgame, both halves)

myedit flipped to every-buffer-hosted (its ROADMAP §E.2); the std work it forced, plus
the review findings fixed the same day:

- **`editor/buffer-client` (ADR-134)** — the protocol's CLIENT half extracted from
  myedit's collab layer: `link-init`/`link-propagate`/`link-fold` (echo suppression,
  foreign-splice transform over in-flight edits, resync fallback), `text-splice` (native
  `%str-splice-diff` under it — the per-keystroke diff went ~40 ms → 0.4 ms on a 300-line
  hosted buffer), `view-parts`, `text-apply-splice` (string holders — registry mirrors).
- **`[:io-write]` is a structured splice** — delta push, transform-ring recorded; a
  streamed log buffer costs subscribers O(line) and no longer invalidates based splices.
- **`buffer-edit-reply` / `buffer-edit-value`** — the read-then-decide edit (kill-region
  shape) as one atomic, totally-ordered round-trip. Built ahead of its editor consumer.
- **OT hole closed: same-origin ring skip** — a client pipelining two splices before the
  first echo folds had its second one double-shifted (the ring transformed it over the
  sender's OWN first splice, already part of its basis). Ring entries now carry origin;
  a based splice transforms only over OTHER origins. Repro'd, fixed, regression-tested
  ("axybcd", not "axbycd"). Latent since the collab track; hot once every buffer hosts.
- **`buffer-sync`** — atomic `[version view]` snapshot (the resync primitive): rebuilding
  after an ambiguous collision pairs text with its version, so queued pushes sort
  themselves instead of replaying onto the rebuilt copy.
- **`require` is a concurrency contract (ADR-136)** — no observer sees a half-loaded
  module; failed loads unwind their in-flight marker.
- **serve: only the CLIENT's `[:down]` closes a session** — a served collab app monitors
  its buffer processes; their corpses must reach `update` (rehost/reshare), not kill
  every attached session.
- **`stop-buffer` is bounded** — 2 s timeout → `:no-reply` instead of parking forever on
  an already-dead process (killing a buffer whose process just crashed froze the caller).
- **KI-10** — `buffer--serve` exposed a `receive` compile cliff at the 13th arm (+65%
  wall / +80% peak on the buffer suite from one trivial arm); worked around by merging
  the two `[:edit]` arms to stay at 12, recorded as a known issue.

## 2026-07-12 — The top-level program is a green process (ADR-135): ping-pong 6.5 → 3.3 µs/RT

Chasing the message-passing latency gap vs Elixir/BEAM. Real baseline (1M round-trips,
startup subtracted): **Brood ~6.5 µs/RT vs Elixir ~0.5 µs/RT, ~13×**. `strace` showed the
smoking gun — **~4.4 futex syscalls per round-trip** (180k erroring), i.e. *not* the
"entirely userspace" path the direct-handoff was supposed to give. Two causes, both fixed.

1. **`enqueue` fired a wake syscall even handing to the current worker.** `Condvar::notify_one`
   is an unconditional `futex_wake` on Linux; the running worker can't be parked on its own
   cv while it's executing the `enqueue`, so that wake is provably useless. Skip it when
   `proc.worker_id == CURRENT_WORKER`. Green↔green ping-pong: 202k futex → ~400 over 100k RT.
   (Shipped separately; a general win for supervisors / gen-servers / any in-worker messaging.)

2. **The benchmark's driver ran as the *root process on the main thread*** — a privileged
   thread that blocks on its mailbox condvar in `receive` and can't do userspace handoff, so
   every leg crossed the main↔worker boundary via futex. Fixed structurally: **the whole
   top-level program now runs as one ordinary green process** (ADR-135, "everything is a
   process", the BEAM model). The main thread spawns it, blocks once on a result slot, and
   translates the outcome to an exit code.

Implementation notes worth keeping:
- **One process, driven form-by-form** (`run_program_body`), so `(self)` is a single stable
  pid across every top-level form (a per-form process would hand each form a different self,
  breaking `(def me (self))` … `(send me …)` — the ring benchmark relies on exactly this).
  A `Suspended`/`Preempted` from any form returns up unchanged; the cursor lives in the
  `Process`, so a top-level `receive` park-captures like any green process.
- **`def` had to be split.** The VM won't body-compile a `def` (a special form), so wrapping
  `(def done (ping 0))` as `(fn () …)` silently deferred to the tree-walker, whose `receive`
  *blocks* — the program deadlocked-slow at the old futex rate. The driver now runs a
  `(def name rhs)`'s **rhs** on the capture path and binds after (`bind_def` re-evaluates the
  trivial `(def name (quote v))` to reuse `def`'s naming / promote-to-shared / reload
  semantics). This was the difference between 6.5s and 3.3s at 1M — found by A/B'ing a
  bare-`(main)` driver (already fast) against the `def`-wrapped one.

Result: the actual `pingpong.blsp` benchmark **6.5s → 3.3s at 1M RT (~13× → ~6.6× vs Elixir)**,
futex 416k → 370; `ring.blsp` 1M hops in ~2.1s. The residual gap is intrinsic to Brood's
design (immutable-data per-message allocation, heap-captured migratable continuations,
per-process heap-isolated message copies) — not traded away. Correctness held across: defn/
def/expr/ns forms, top-level macro-then-use, multi-file shared defs, `def`-with-receive-RHS,
let-it-crash workers, background children, empty/value programs, clean error→exit-1. REPL and
`--test` paths are untouched (they keep the main-thread evaluator). Full suite 777/777 green.

## 2026-07-12 — Kill an O(n²) landmine in `string->list` (and the `(str acc …)` / `char-at`-scan family)

Chasing the last-place wider-range benchmarks (`../brood-benchmarks`: json super-linear,
base64 1.3 GB RSS). The base64/json fixes had been rewritten to index a **code-point
vector** — `(into [] (string->list s))` + O(1) `nth` — to escape `char-at`'s O(i) UTF-8
walk. But the benchmarks stayed quadratic. Root cause, found by bisection:

- **`string->list` was itself O(n²).** It built each char with `(substring s i (inc i))`,
  and `substring` walks to char boundary `i` each call — so `(into [] (string->list s))`
  was O(n²) to *construct*, silently defeating the whole char-vector rewrite. Reimplemented
  over the native `string-split s ""` (one O(n) `chars()` pass). This one line is the big
  win — it fixes json parse, base64/hex, and every caller of `string->list`.
- **`(reduce … (bytes-value))` was O(n²).** `fold`/`reduce`/`map` walk with `first`/`rest`,
  and `(rest bytes)` **copies** all-but-first byte — O(n) per step. Fixed generally in
  `seq` (prelude): a `bytes` value is realised to a list once via `bytes->list`, so every
  sequence op over bytes is O(n). (base64 decode-sum: 1.37 GB → 96 MB.)

Then swept the same two anti-patterns — per-char `char-at`/`substring` scans, and
`(str acc …)` accumulation (each `str` copies the whole accumulator) — across `std/`:
- **`std/csv.blsp`** — parser now indexes a code-point vector (`nth`, not `char-at`), and
  a field is a reversed char-list joined once (was `(str field-acc c)` per char).
- **`std/net/tcp.blsp` + `std/net/http.blsp`** — `tcp-drain*` and `http--collect` cons the
  TCP chunks and `(apply str (reverse …))` once at close (was `(str acc d)` per chunk,
  O(body²) on a large response). (The request-read path was already chunk-listed.)
- **`std/url.blsp`** — `percent-encode`/`percent-decode`/`query-encode` accumulate pieces
  in a reversed list and scan a code-point vector.
- **`std/format.blsp`** — `count-newlines` via native `string-split` (was per-char `substring`).

Results: json 2000 **2.5 s → 0.93 s**, 5000 **12.7 s → 2.36 s** (was O(n²), now ~linear);
base64 50k **1.39 s / 1.3 GB → 0.25 s / 106 MB**; csv-parse now ~linear. Full suite
(777 Rust + Brood) green; `nest check` zero warnings; rustfmt clean.

Deliberately **not** touched (low value / high risk): the `std/format.blsp` pretty-printer
(`render--*` interleaves `(str acc …)` with `(string-length acc)` alignment reads, and it
formats bounded per-form source), the prelude `format`/`datetime` format-string loops
(short format strings), and the `std/editor/*` + `std/tool/sexp.blsp` `char-at` buffer
scanners (large, sensitive surface; assess whether they scan ropes or extracted strings
before converting).

## 2026-07-13 — Share closure arms behind an `Arc` (`ring` 2.02 → 1.50 s, ping-pong ~18%)

Attacking the message-latency gap (`../brood-benchmarks` `ring`: 200 procs × 5000 laps =
1M hops, ~7.4× Elixir; `pingpong`: 100k round-trips). Profiling put ~13% of `ring` in
per-`receive` **matcher-closure churn**: the `receive` macro expands to
`((%receive (fn (msg) …) …))`, so every message builds a fresh closure — and each build
deep-cloned the arm `Vec` (params/optionals/body) out of the template cache, plus the
matching alloc/free/memmove traffic that fed the GC.

Fix: `Closure.arms` and `ClosureTemplate.arms` are now `Arc<[ClosureArm]>`, so
`build_closure` hands each instance an `Arc::clone` of the cached template's arms — a
refcount bump, not a copy. The GC rewrites arm handles in place: `Arc::get_mut` on the
unique-owner paths (the hot **minor flush** and `alloc_closure` pass-through fill) and
`Arc::make_mut` on the two rare relocating paths (prelude freeze, RUNTIME compaction).

The correctness argument that makes this safe under the moving collector: a *shared* arms
can come **only** from the RUNTIME-keyed template cache, so it holds only RUNTIME handles —
which a minor collection never relocates. Therefore the minor-flush hot path can `get_mut`
and **skip a shared arms entirely** (nothing to flush, no un-sharing clone), and only the
rare def-churn compaction needs `make_mut` (the same `gen_version` bump that drives
compaction also invalidates the template cache, so fresh closures re-share afterward).
One latent trap fixed: `Closure::clone` now shallow-shares the `Arc`, so `name_value`
(the `defn` naming copy) must deep-copy the arms — otherwise two live LOCAL closures could
share one arms and defeat the skip invariant.

Verified: full suite **777/777** green (incl. the 115 s in-language `brood_suite_passes`),
doc-tests pass, rustfmt clean, `cargo clippy -p brood` clean on the touched files. GC
stress (`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`): the compaction + shared-arms and
`name_value`-churn stress cases pass, plus `vm_closures` 13/13. Perf (clean release-fast
binary): `ring` **2.02 → ~1.50 s (~26%)**, `pingpong` **0.34 → ~0.28 s (~18%)** — the win
is bigger than the 13% churn because killing the arm-`Vec` clone/drop also cuts the
allocation/GC pressure that fed it. Files: `core/value.rs`, `eval/mod.rs`, `core/heap.rs`,
`process/message.rs`.

## 2026-07-13 — Unboxed-i64 worker covers tail self-calls: `ackermann` 4.0 → 0.36 s (7/7 → 3/7)

Chasing the last-place benchmarks (`../brood-benchmarks`). Ruled out four of the five 7/7 rows as
structural (regex is driver-loop-bound not matcher-bound — its parse is already cached, so the FRONTIER
note was stale; sieve/nbody/persistent-map are intrinsic immutable-model costs). Tried the fast-hasher
first (SipHash→a splitmix64-finalized in-house mixer for `hash_value`): correct, but **reverted** — a
lookup microbench showed hashing is <5 % of a map op (dispatch + CHAMP alloc dominate), so it didn't
clear the "demonstrable broad win" bar.

Then **profiled** `ackermann` instead of guessing: 98 % in `brood_jit_arm_8` — it was already JIT'd, just
on the **boxed** path, not the unboxed-i64 register worker that took `fib` 227→54 ms. Traced why: the i64
worker's subset checker (`i64_has_self_call` / `i64_value_ok`) and lowering only recognized `Node::Call`
(a *non-tail*, argument-position self-call, like `fib`'s). Ackermann's recursion is in **tail** position
→ `Node::SelfCall`, which the subset never matched, so `ack` fell to the boxed path. Secondary blocker:
`I64_DEPTH_LIMIT = 1400` was stale — sized for the removed (ADR-100) coroutine stacks; a green process,
incl. the top-level program, now runs on the 16 MiB worker-thread stack, and `ack`'s ~4093 non-tail
depth overflowed 1400 and depth-bailed to boxed anyway.

Fix (pure kernel, `eval/compile/jit_lower.rs`):
- `i64_has_self_call` recurses *into* a `SelfCall`'s args (to find a genuinely-recursive nested `Call` —
  Ackermann's inner `(ack m (- k 1))` lives inside the outer tail `(ack (- m 1) …)`) but a bare
  `SelfCall` doesn't itself count, so a **pure-tail** int recursion (`loop`/`collatz`) still stays on
  the faster self-tail-loop path, not this recursive worker.
- `i64_value_ok` accepts `SelfCall` (validates `nargs` in-subset args); `lower_i64_value` lowers it
  identically to a self `Call` (the worker has no tail-loop — a tail call recurses natively, bounded by
  the depth cap → deopt).
- `I64_DEPTH_LIMIT` 1400 → 32768 — stack-safe (measured ~55 B/frame; >2× margin at a pessimistic
  ~200 B/frame on the 12 MiB budget), covering deep non-tail int recursion the old cap punted to boxed.

Result: `ackermann` **4.02 → 0.36 s (~11×)**, now on `brood_jit_i64w_8` — by compute (~0.33 s) it's
**7/7 → 3/7**, past Node/Clojure/Ruby/Python, behind only .NET and Elixir. Broad, not one-off: any mixed
tail+non-tail int recursion now rides registers. Full suite **777/777** (incl. the 4-engine differential
fuzzer); 4 engines agree bit-for-bit on `ack`/a second mixed shape; no regression on any JIT row
(loop/collatz slightly *faster* — more depth stays on the fast path); runaway 5 M-deep recursion still
raises a **clean error, not SIGSEGV** (depth-bail → boxed drain). rustfmt clean.

## 2026-07-14 — nbody 6.65 → 1.67 s (~4×): bodies list→vector + variadic MakeVector JIT + selective float carry

`nbody` was **7/7 (dead last, ~40× Elixir)**. The design plan (`jit-float.md`
§Float-across-calls) assumed the gap was boxed floats across call chains. It wasn't —
the dominant cost was the **data structure**. The benchmark stored the 5 bodies in a
`(list …)`, so `(f b i k)` = `(nth (nth b i) k)` did an **O(i) linked-list walk**,
re-walked on every field read. Every *other* language's port uses an O(1)-indexed
container (Node/Ruby/.NET arrays, Elixir a **tuple** `elem(b,i)`), so Brood's list was
a mis-transcription. A Brood **vector** is the faithful equivalent.

Three changes, in order of impact:
1. **Benchmark fix (`bench/brood/nbody.blsp`, brood-benchmarks): bodies `(list …)` → a
   vector.** Pure O(1) indexing — **6.40 → 1.94 s on the VM alone** (~3.3×), checksum
   unchanged (`-169078071`). A fairness correction, not a language change.
2. **Variadic `MakeVector(n)` JIT lowering** (`jit_lower.rs` + `jit/mod.rs`). Was capped
   at `n == 2` (bintree's `[a b]`); a wider literal (nbody's `[vx vy vz]` / 7-body
   rebuild) bailed the whole arm. Now the `n` elements are boxed into a per-site Cranelift
   stack slot and built by a new `brood_rt_make_vector_n(heap, out, elems, n)` helper —
   `alloc_vector` only *grows* the slab (never collects), so the staged bytes can't go
   stale mid-call (same discipline as `make_vector2`). `newvel`/`advance-body`/`momentum`
   now lower.
3. **Selective per-slot register carry** (`jit_lower.rs`). The float-carry machinery
   required *every* self-call arg slot to be Int/Float; nbody's `newvel`
   (`b:handle i:int j:int vx/vy/vz:float`) has a handle slot → it bailed entirely. Carry
   is now **per-slot** (`Vec<Option<(Variable,bool)>>`): scalar slots ride registers, the
   handle slot stays on the (rooted) frame. Sound — a scalar in a register is invisible to
   GC across a call safepoint, and a deopt always restarts from the frame's last-SelfCall
   inputs (the `roots` stores are kept; the per-read tag-check guards a mistyped profile →
   deopt). Subsumes the old all-scalar `int_carry_eligible` path bit-identically (removed).

On the vector benchmark the JIT (2+3) adds ~14% over the VM (**1.94 → 1.67 s**). Net
**6.65 → 1.67 s (~4×)**, nbody **7/7 → ~11× Elixir** (off last place). Verified: jit
28/28, differential `engines_agree_on_corpus` 2/2, every scalar/HOF bench bit-identical
to `BROOD_VM=0`, nbody exact at N=50000, warning-free build.

Still ahead: **Layer B** (typed cross-arm float ABI — unboxed f64 across `Call`
boundaries) is now a *larger* fraction of the reduced runtime and remains the deep
future win (`jit-float.md`). Branch `perf/jit-nbody-float`.

## 2026-07-14 — nbody 1.25 → 0.82 s (JIT now earns its keep): fix vector-read + float-handle deopts

After the list→vector + bind-once benchmark fixes (6.65 → 1.25 s), the JIT was
**net-neutral** (jit ≈ no-jit ≈ 1.39 s). `BROOD_DEOPT_TRACE` instrumentation showed
`newvel` and `advance-body` **deopting on ~every call** (~250k each, ≈498k total) — the
JIT ran native, bailed partway, and finished on the VM. Two root causes, both fixed:

1. **Vector reads of a >2-element vector deopted.** `INLINE_VEC_CAP = 2`, so nbody's
   **7-element** body vectors are heap-backed, and `inline_vec_ref` (constant-index
   `(nth v k)`) deopted on the non-`Inline` discriminant — every field read fell to the
   VM. Fix: on the non-inline branch, fall back to the general `brood_rt_vector_ref`
   helper (handles any storage; only errors on a bad index) instead of deopting. Keeps the
   fast inline path for `bintree`'s 2-element nodes.
2. **Float arithmetic on a vector-read `Handle` deopted.** `(nth v k)` yields an
   `Op::Handle` (type-erased); `op_is_float(Handle)` is `false`, so `(- (nth bi 0)
   (nth bj 0))` took the *integer* path → `as_int(Handle)` tag-checks `Int` → it's a
   `Float` → deopt. Fix: `as_f64(Op::Handle)` now tag-checks `Float` and extracts (deopt
   only if genuinely not float); in a float-context arm (`has_float_slot`), `Handle`-operand
   arithmetic routes to the float path. A wrong guess is a deopt, never a miscompile; a
   right guess yields `Op::Float`, which `store_op` marks float so the rest of the chain
   stays unboxed. Also implemented float `/` in `emit_float_arith` (was `None`→bail; guard
   a zero divisor → deopt, matching the VM's `(/ x 0.0)` error).

Result: `newvel` now runs **fully native** (deopts 498k → 249k; VM `prim2_inline` 36M →
5.2M as that arithmetic moved to native). nbody **1.25 → ~0.82 s** — **6.65 → 0.82 s
overall (~8×)**, from ~40× Elixir to ~5×. Verified: full in-language suite **2730/2730**,
jit 28/28, differential fuzzer 2/2, all 13 numeric benches bit-identical to `BROOD_VM=0`,
`BROOD_GC_STRESS`+`GC_VERIFY`+`JIT_VERIFY` clean on nbody, `bintree` unregressed, fmt clean.
Residual: `advance-body` still deopts (~249k) — it has no float *param*, so the
`has_float_slot` gate misses it; catching it needs a float-context signal that survives
`(nth …)`/call-return type erasure (cross-arm return typing or a float-global-aware gate),
without regressing int-vector arms (matmul/nqueens) — deferred. Branch `perf/jit-nbody-float`.

## 2026-07-14 — Regex compiles to a lazy DFA (`regex` 1.03 → 0.69 s; catastrophic patterns now linear)

`std/regex.blsp` matched by walking the AST with a **CPS backtracker** — every step
allocated a fresh continuation closure, and a pathological pattern (`(a*)*b`) backtracked
exponentially. "Compile it like Erlang does" (the roadmap lever, the user's ask): the parse
was already memoised, but *matching* re-interpreted the tree. Now the pattern compiles once
(memoised) to a **Thompson NFA** — a flat vector of states (`:char`/`:split`/`:bol`/`:eol`/
`:match`) built functionally, with `*`/`+` back-edges reserved-then-`assoc`'d in — and is
simulated closure-free.

First cut (NFA + integer-bitset state sets, `bit-or`/`bit-shift`/`bit-positions`) was
**correct but 3.4× *slower*** (3.3 s): the per-char simulation was call-bound — 28 M Brood
calls for 20 k matches (`vm-perf`), because scanning live states with a helper call per state
(and a `reduce` lambda + `bit-positions` vector allocated every char) is far more work than
the old greedy inner loop. The fix that actually won is a **lazy DFA**: memoise each
`(state-set, char) → next-set` transition (split-closure only, so it's position-independent —
`^`/`$` fire only in the full closure run once at the start/accept boundaries) in a `Table`
living in the compiled object. After warmup every character is a single `table-get`, not a
~30-call NFA scan.

Result: `regex` **1.03 → 0.69 s** on this machine (~1.5× overall; the *matcher* portion
~0.70 → 0.33 s, ~2.1×), checksum 10000, all 14 tests + 19 added edge cases green (incl.
`(a*)*b` on 24 `a`s — **linear now, not exponential**), full suite **777/777**, `nest check`
clean. Pure Brood, no new kernel primitive — the dogfood-correct "compile" the roadmap asked
for. **Does not clear 7/7**, and honestly can't from `regex.blsp` alone: the benchmark's
caller loop contains the `matches?` call so it never JIT-tiers (~0.33 s of the 0.69 s is the
interpreted `go` loop), and each `delta` still pays the `Table`-get overhead (the same
registry-lock + `Arc`-clone + deep-copy tax `sieve` hits). The gap to Clojure (122 ms) is
interpreter overhead per match, not matcher algorithm — closing it needs cheaper `Table` ops
(the `sieve` levers) and/or call-bearing loops tiering, both kernel work, both shared with
other rows. The DFA is the right architecture regardless (linear-time guarantee).

## 2026-07-15 — Table throughput: lock-free registry + fast scalar hash (and why `sieve` stays 7/7)

Chasing `sieve` (7/7, ~19× Elixir — a `Table` of composite marks). Ablation localised the cost
precisely (perf is unusable in the sandbox, so this was measured by short-circuiting each stage of
`table.rs::has` behind a read-once toggle):

- The registry `Mutex` + `Arc<Store>` clone per op: real but small.
- `heap.hash_value` built a fresh **SipHash** (`DefaultHasher`) to hash *one integer* — ~40 ns/op of
  pure overhead on every table/map op.
- The store `Mutex` and `find_idx`: **~free** (uncontended lock is cheap; bucket size 1).
- **`HashMap::get` on the 3 M-entry map: ~183 ns/op** — cache misses probing a big (~32 B/entry) table.

Two general fixes landed (help *every* `Table` user and the CHAMP maps — regex DFA memo, `wordcount`,
`json`, any process state):
1. **Lock-free registry** (`table.rs`): the `Mutex<HashMap<id, Arc<Store>>>` became an append-only
   `boxcar::Vec<Store>` indexed by `id-1` (ids dense, never reused). A handle resolves with one
   lock-free indexed read + a `'static` borrow — no registry mutex, no `Arc` clone per op, and the
   *global* serialization point every process's every table op hit is gone. `table-drop` tombstones
   in place (data cleared); no slot reuse, so no ABA — a stale handle still errors. Fine given the
   design intent (tables are app-lifetime).
2. **Fast scalar hash** (`heap.rs::hash_value`): int/bool/nil keys — the hot case — take a splitmix64
   finalize instead of SipHash; compound keys are byte-identical to before. Plus an identity hasher on
   the store's `HashMap<u64, …>` (its key is already a structural hash — no need to SipHash it again).

Result: `sieve` **1.22 → 1.07 s** (~12 %), `wordcount` ~0.17 → 0.13 s (~20 %), `persistent-map`/`json`
flat. Full suite **777/777**, rustfmt clean.

**`sieve` stays 7/7, and the data structure isn't why.** A follow-up microbench nailed the floor:
**every builtin call costs ~90–120 ns of VM dispatch** regardless of the work it does (a no-op
`table-count` on an empty table: ~86 ns; `string-length`: ~120 ns; vs ~26 ns for a call-free loop
iteration). `sieve`'s ~3.8 M table ops therefore have a **~0.33 s floor** before *any* storage work —
already 2.4× Clojure's whole run (138 ms). So neither a dense int-array (Phase 2, would cut the
cache-miss half) nor anything in `table.rs` can clear `sieve`'s 7/7; the same interpreter-dispatch
ceiling that pins `regex`. The genuinely general lever behind *both* is cutting builtin-call dispatch
cost (JIT-inlining hot builtins so `table-*`/`string-*` don't pay the ~90 ns crossing). That's the
next target, not more `Table` micro-tuning.

## 2026-07-15 — Register worker learns `throw`: `errors-deep` 0.28 → 0.07 s (~4×, 5/7 → ~2/7)

`errors-deep` (throw from 50 non-tail frames, catch at the top, ×50k) looked like "unwind
cost" but wasn't: ablation showed throw+catch with **zero** frames between is ~free, and the
cost scaled linearly at ~96 ns per frame — with `BROOD_PERF_STATS` showing `jit_native=0`
and 2.6 M interpreted `call_ic_hit`s. The real cause: **the `throw` call knocked `descend`
out of the unboxed-i64 register worker's subset**, so all 2.5 M frames were *built* on the
interpreted VM call protocol (the noraise twin runs the worker: 0.06 s). The unwind itself
was always cheap.

Fix (kernel, `jit_lower.rs` + `jit/mod.rs`): the i64/f64 register worker now lowers
`(throw <scalar-expr>)` — evaluate the payload in registers, call a new
`brood_rt_i64_throw(heap, bits, is_float)` callback, store the returned sentinel, and unwind
through the existing `poisoned` block. Sentinel **3** = the callback verified the global
`throw` still binds the builtin, boxed the immediate payload (`Value::Int`/`Float` — never a
heap handle, so GC-safe by construction), and parked the error in `jit_pending_error`; the
wrapper maps it to **outcome 3** (both `jit_tier` call sites already propagate a parked
error). Sentinel **1** = a user *redefined* `throw` → no park, plain deopt: the VM re-runs
the (pure-up-to-the-throw) arm and calls the redefinition — late binding stays exact, checked
per throw at runtime, not frozen at compile. The worker gained a `heap` param (rides along
untouched except by the throw callback); `do` lowering now emits **every** form, not just the
last, so a non-final `(do (throw x) …)` still raises (pure dead values DCE away).

Result: `errors-deep` **0.28 → 0.07 s** (compute ~40 ms — past Ruby/Node/Python, behind only
Elixir's 4.4 ms). Verified: 3 engines agree on the benchmark; payload identity across the
native unwind (int + float workers); non-final-`do` throws fire; 40 k-deep recursion
depth-bails to boxed and still catches; redefining `throw` after the arm is hot gives the
tree-walker's exact answer (827). New `:serial` cases in `tests/jit_throw_catch_test.blsp`.
Full suite 776/777 + the one failure (`remote-spawn-sync: no reply`) reproduced as a
CPU-starvation flake — it passes alone in 14.5 s vs the 127 s it took under the fully
parallel run. No regression on the other i64-worker rows (fib/ackermann/collatz/loop flat
in a binary-verified A/B). rustfmt + `nest check` clean.

## 2026-07-15 — `persistent-map` off 7/7 by transcription; two JIT hypotheses tested and refuted

(NOTE for merge: the sibling perf branches — `perf/regex`, `perf/table`, `perf/errors` —
each appended their own dated entry at this same anchor; on merge, keep ALL entries in
date order.)

`persistent-map` (RMW churn on a 50k-key map, 612 ms published, 7/7): the port hand-wrote
`(assoc acc key (+ (get acc key 0) d))` — two CHAMP descents — while Elixir's port uses the
one-call `Map.update/4` and `wordcount` already uses Brood's own fused idiom. Transcribed the
step to **`map-int-add`** (roadmap lever 1): **0.71 → 0.16 s** locally (~4.4×), checksum
bit-identical on all three engines; harness-scaled ≈ 138 ms → past Clojure (285 ms), 7/7 →
6/7, ~1.2× from Elixir (118 ms). Benchmark-repo change only; no kernel change.

The interesting part is what the kernel investigation **refuted**. Ablation first showed the
un-transcribed loop paying ~0.9 µs per `get` and ~1.5 µs per `assoc` (dispatch layers: the
polymorphic prelude `get`/`assoc` closure + `map?`/`vector?` + the kernel native), while
`map-int-add` lands at ~0.4 µs. Then two hypothesized JIT levers for the remaining floor:

1. **"The profitability gate blocks defn-style tail loops with calls"** — relaxed the gate so
   a tail `Inst::Call` whose head == the arm's own `dbg_name` counts as a self-loop. Measured:
   **zero change** on persistent-map/sieve/regex/wordcount/json.
2. **"Defn-style tail loops never get hot"** — the `Inst::Call`-tail inline fast path in
   `exec_chunk` has no back-edge escape (unlike `SelfCall`'s every-256 escape), so a loop
   entered once can never reach `jit_tier`'s threshold. Added the twin escape. Measured:
   **zero change**.

Tier-tracing explained why: the LINMAP rewrite (`docs/linear-map-accumulator.md`) had already
turned the benchmark's accumulator into a private Table (`map-int-add` → `table-incr`) as
`go/linmap-loop__79`, whose letrec-style `SelfCall` loop **already lowers, installs, and runs
JIT-native** (149 tier entries, valid pointer, current epoch — the `jit_native` perf counter
simply doesn't cover the `vm_run_bc` try_jit path, which had me chasing a ghost). The loop was
native all along; the 0.16 s floor is the per-iteration native-call + Table-op protocol — the
same shared floor `sieve` and `regex` sit on (see the 2026-07-15 Table entry's ~90–120 ns
builtin-call measurement). Both experimental changes reverted; the gate is correct as-is.
Deferred roadmap levers (assoc-path node alloc; a general fused `update`) remain valid for
non-linmap map workloads.

## 2026-07-15 — Dense Table storage + table-op prims: `sieve` 0.88 → 0.15 s (~6×, at Clojure's heels)

The two remaining dead-last rows (`sieve` 284×, `regex` 208×) share the measured floor:
per-op Table cost × per-op call dispatch. Both halves cut, in three layers:

**1. Dense int-key storage (`table.rs`).** A `Store` is now `Storage::Dense { vals:
Vec<DenseVal>, count }` — int keys in `[0, 2^23)` with scalar (`nil`/bool/int) values
index a flat array directly: no structural hash, no bucket probe (the 3M-entry HashMap
probe was ~183 ns of cache misses per op), no `Message` key clone. `DenseVal::Empty` vs
`::Nil` keeps `table-has?` exact for stored nils. Every table starts dense; the first
out-of-shape op (string/negative/out-of-range key, non-scalar value, too-sparse grow —
`new_len ≤ 64·count + 4096` bounds RSS) migrates one-way to the original hashed map with
every entry preserved. Alone: `sieve` 0.88 → 0.33 s and **RSS 417 → 60 MB**.

**2. Table ops as prims.** `table-has?` / 2-arg `table-get` join `PrimOp`
(exec-arm-handled like `Cons`/`VectorRef` — same `check_key` and error text as the
natives, non-Table operand defers for the exact type error); `table-put` gets the first
**`PrimOp3`/`Node::Prim3`/`Inst::Prim3`** (a new 3-operand inst: operands on the stack,
same epoch-guard + `resolve_prim3` + `prim3_dispatch_rooted` fallback discipline).
Removing the per-op native `Call` also removes the last calls from `sieve`'s loops, so
they keep full register carry.

**3. JIT lowering.** All three lower as one runtime-callback call
(`brood_rt_table_has/get2/put` — vector_ref-style word-triple ABI) with a 3-way status:
0 = done, 1 = deopt (VM owns the exact type error), 2 = a real error parked in
`jit_pending_error` → the arm's error block (outcome 3). The callbacks may allocate (a
compound `get` reconstructs) but never collect, so register-held handles stay valid.
`TableGet`/`Prim3` count as spill producers.

Result: `sieve` **0.88 → 0.15 s (~6×)** — harness-scaled ≈ 0.14 s, right at Clojure's
144 ms (the 7/7 escape is a re-run coin-flip); `regex` 0.69 → ~0.64 s (its residual is
the interpreted matcher loop, not the memo). Verified: 3 engines bit-identical on every
table-heavy row + the full gauntlet; 42/42 table tests (7 new dense/migration cases:
stored-nil vs Empty, count bookkeeping, all three migration triggers, incr across
migration + its type error, snapshot); 22-case dense edge script; **redefining
`table-put`/`table-has?` after the loops are hot dispatches the redefinition on both
engines** (epoch guard exact); GC-stress clean on sieve+regex.

## 2026-07-15 — Regex compiles harder (re:compile discipline) + the JIT learns keyword `=`

Finishing the `regex` dead-last work. Two layers, one found-bug:

**1. `std/regex.blsp`, the Erlang `re:compile` discipline** — everything pattern-constant
moves into the compiled object, nothing is recomputed per match call: `:matchbit` (the
accept bit, so accept tests are plain `bit-and`), `:startfull` (the full start closure —
position-independent for every n > 0), `:startmid` (the search injection set), and
`:exitmemo` (accept-closure memo: final set → boolean, computed with the (1,1) position
sentinel). The input converts once to a vector of int codepoints (`regex--codes`), so the
per-char loop is pure prims — vector-ref, int arith, `table-get` — plus the self tail
call; the n = 0 edge (both anchors fire at position 0) recomputes live. The old
per-char-`Call` `regex--delta` is gone (miss path → `regex--delta-slow`).

**2. The JIT learns interned-immediate `=` (`eq_dispatch`).** Deopt-tracing showed the NFA
walkers deopting on **every keyword compare** (`(= (get st :t) :split)` — 600 k deopt+
re-runs): the `Eq` lowering was int-only. New runtime-dispatched equality on a
type-erased operand: Int×Int → payload compare (the same two tag-checks the old path
paid), either side Sym/Keyword → interned identity (tags equal AND ids equal — a
keyword equals nothing but its same-tag same-id self, never numerically coerced),
anything else → deopt (the VM owns numeric coercion and structural equality). New
`TAG_SYM`/`TAG_KEYWORD` in `jit_layout`, pinned by the layout test. This is a *general*
win — keyword dispatch over tagged maps is idiomatic Brood everywhere (`pipeline`
0.07 → 0.06 s came along for free).

**The found-bug (caught by the suite, JIT-only):** a Sym/Keyword payload is a `u32` — the
HIGH half of the payload word is **undefined padding**, so the first `eq_dispatch` cut
compared garbage and equal keywords could compare unequal (7 sexp paredit tests failed
under JIT, passed under `BROOD_NO_JIT=1`). Fix: mask both ids to the low 32 bits. A
reminder that any new word-level compare of a sub-word payload needs the mask.

`regex` **0.65 → 0.59 s** local. Honest bottom line: still 7/7 — the remaining ~0.45 s is
the *benchmark harness loop* (interpreted `go` with calls, ~0.33 s) + the per-call codes
conversion (~0.10 s), i.e. the cross-arm call ceiling, not the matcher (long-string
matching is now memo-lookup-bound). The next general lever is making Brood→Brood calls
from native code cheap — a kernel project of its own. Suite 777/777; 3 engines
bit-identical across the gauntlet; fmt + `nest check` clean. (Mid-run the disk filled —
7 scratch worktrees × multi-GB `target/`s; reclaimed ~32 GB by deleting their build
artifacts. The worktrees themselves are merged and disposable.)

## 2026-07-15 — The big one, increment 1: native→builtin calls get an IC fast path

The cross-arm call ceiling, measured precisely (10M-iteration microbench from a JIT'd
loop): a Brood→Brood call via the fast-link is already ~26 ns — but a call to a **Rust
builtin** cost **~55–75 ns** (`string-length` 55, `char->int` 75), because a Native
callee never entered the call IC: `jit_dispatch_call` fell to the slow path — an
`env_get` per call + the full `dispatch` (passthrough loop, `apply`-unfold check,
re-dispatch) — every single time.

Fix (pure `jit_dispatch_call`, no lowering change): the call IC now caches Native
callees as arm-less entries; a hit invokes the fn pointer directly on the staged
(rooted) args — one arity check, no `env_get`, no `dispatch`. `apply` has a real
native body so even it is exact; dynamic heads are never cached (call direct, uncached).

Builtin calls from native code: **55→38 ns / 75→58 ns**; `str`-heavy loop −17%.
Free riders: `strings` 0.13→0.04 s, `wordcount` 0.11→0.09 s. Brood→Brood unchanged
(0.40/0.50 s microbench flat); full gauntlet 3-engine bit-identical; suite 777/777.

Remaining rungs on this ladder (the BEAM picture: args in X registers, patched direct
calls, no per-frame zeroing): batch arg staging (one stack-slot FFI instead of
`brood_rt_push` × argc), a flat per-site native-pointer cell to kill the IC RefCell
borrow, and skipping the nil-fill for definitely-assigned frames. Each is a measured
10–20 ns; none taken yet.

## 2026-07-15 — The big one, rungs 2–4: batch staging, native flat cell, memset frames

Three more cuts on the native-code call path, on top of the morning's IC fast path:

**Batch arg staging (BEAM X-register style).** A call from JIT'd code staged each
operand with its own `brood_rt_push` FFI (argc round-trips, each with a capacity
check). Now every operand is written to a per-site staging STACK SLOT with plain
stores and staged onto `roots` with ONE `brood_rt_push_n` (reserve + memcpy). The
elided-head tail-call case prepends the IC-resolved callee at slot 0.

**Native flat cell.** The IR-readable `FastLink` mirror now carries builtin callees:
`nslots == u32::MAX` marks a native link whose `code` field holds the `NativeFnPtr`
bits (arity pre-validated for the site's exact argc at publish, so the hot path has
no arity check). The Track B hit block branches on the marker: a native hit calls
`brood_rt_call_native_fl(heap, out, func, stage_ptr, argc)` — the trampoline reads
the args straight from the staging slot, no `env_get`, no dispatch, no IC borrow.
The roots-staged copies anchor the arg values for any GC the native triggers
(matching the VM's operands-stay-rooted discipline) and the trampoline drops them
on return. Published from `jit_dispatch_call`'s native fill/hit paths; invalidated
by the same epoch/sym/argc guards as Brood links.

**Memset frame fill.** `extend_roots_to_nil` — the frame nil-fill on every call in
both engines — was `Vec::resize`'s per-slot 24-byte clone loop; all-zero bytes are
a valid `Value` (`Nil`), so it's now one `write_bytes`. (The "skip the fill via
definite-assignment" idea is unsound as stated: `roots` is traced to its length, so
unfilled slots would feed stale garbage to the GC. The memset is the safe version.)

Cumulative microbench (10M calls from a JIT'd loop, vs this morning's baseline):
`string-length` 55 → **34 ns**, `char->int` 75 → **52 ns**, `str`-heavy loop
2.03 → **1.50 s** (−26%), 3-arg Brood→Brood 0.54 → 0.47 s. Full gauntlet (21 rows)
3-engine bit-identical; GC-stress + `BROOD_JIT_VERIFY` clean on the call-heavy rows
(the staging discipline changed — this was the load-bearing check); suite 777/777.

## 2026-07-15 — `sqrt` inlines as `fsqrt`: nbody 0.74 → 0.54 s (kills the last coin-flip 7/7)

The fresh harness run left two dead-last rows: `regex` (real) and `nbody` — last by
**0.5%** against Python, flipping run to run. The roadmap's deferred sqrt lever settles
it: nbody's ~2M `(sqrt dsq)` calls each paid a Brood→Brood call INTO an interpreted
wrapper (the prelude `sqrt` guards negatives with an `error` call, which keeps its own
tiny arm off the JIT — the gate bails a no-loop arm with a call).

`PrimOp1::Sqrt`, special-cased in `resolve_prim1` on the untouched PRELUDE `sqrt`
closure (the `nth` → `VectorRef` discipline — a user redefinition cleanly disables it,
epoch-guarded). The inline covers ONLY x > 0: one IEEE `fsqrt` in the JIT (with an
`Op::Float`/`Op::Int` register path and a runtime tag-dispatch path for type-erased
operands), `f64::sqrt` in the VM exec arm — both correctly rounded, bit-identical to
the wrapper's `%f64-sqrt`. Zero, negatives (the wrapper's error), NaN, and bignums
deopt/fall back to dispatching the real wrapper, so semantics are untouched.

nbody **0.74 → 0.54 s** (≈520 ms compute vs Python's ~715 — decisively 6/7 now; only
`regex` remains last-of-seven). Checksum bit-identical on all three engines; sqrt edge
script green (0 → 0.0, negative → error, int operands, redefinition-after-hot wins on
the hot loop); GC-stress clean; 16-row gauntlet 3-engine identical; suite 777/777.

## 2026-07-15 — `std/json`: the parser goes int-codes, the encoder goes emit-list (0.39 → 0.30 s)

The regex playbook applied to `json` (363 ms, 303×, the top remaining multiple). The
parser already indexed a char VECTOR (the old O(n²) `char-at` fix) — but of 1-char
STRINGS, so every per-char test (`(= c "{")`, `digit?` via `includes?`) was a
string-equality/scan through the slow dispatch path. The whole parse section now runs
on **integer code points** (-1 = EOF sentinel): every per-char test is an int prim,
digits are a range compare, literals match by code, escapes/`\uXXXX` produce codes,
and a string body assembles once at the close quote via `int->char` (which the module
predates — its `read-string`-based `cp->string` workaround survives only in the
encoder's control-escape table). Number validation walks the code vector directly; a
token string is built only when it parses.

The encoder's `join`+`map` per node (a lazy seq-view + realize per object, and a
re-copy of the text at every enclosing level) became a single-pass **emit into a
reversed fragment list** with one `(apply str (reverse …))` at the top.

`json` 0.39 → **0.30 s** (parse portion −37%; the encoder's residual is
allocation-bound — `map-pairs`/`number->string` per node — left for a future pass).
30/30 json tests + 9 new edge checks (surrogate pairs, strict-number rejections,
control-char encoding, round-trips); GC-stress clean; 3 engines bit-identical;
suite 777/777. Still 6/7 — native C parsers are the field here; the multiple drops
~25% and the row stays pure Brood.

## 2026-07-15 — BEAM-style reduction batching on the JIT loop back-edge (collatz −35%)

Chasing `loop`'s anomalous ~9.4 ns/iteration: every JIT'd self-tail back-edge paid a
`brood_rt_tick` **FFI + two TLS ops per iteration** (the entry-time `in_capture` gate
went dead when ADR-135 made the top level a capture-mode green process — every loop
takes the poll path now) plus the hoisted-global epoch-guard load. BEAM doesn't check
per reduction; it burns a budget in batches.

The back-edge now decrements an **in-register countdown** (`TICK_BATCH = 128`): while
nonzero, resuming the loop is one sub + branch — no FFI, no TLS, no guard load. The
poll block (every 128th iteration) refills the countdown, runs the epoch guard (a
rebind is observed within one batch — the guard's documented "eventually" contract),
and settles the reduction account with a new `brood_rt_tick_n`/`tick_capture_n`
(burns the whole batch, so the budget depletes at exactly the old rate — scheduler
fairness unchanged; only the check granularity coarsens, bounded at 128 iterations).
Frame stores stay per-iteration: deferring them to the poll would make body-deopts
(arith overflow) resume from a stale batch-start state — replaying up to 127
iterations, which duplicates side effects in any effectful loop. Unsound; not taken.

collatz 0.17 → **0.11 s** (−35%), mandelbrot 0.29 → 0.24, primes 0.08 → 0.07, loop
0.31 → 0.28 (its residual is the per-iteration frame stores, deliberately kept).
Concurrency rows flat (spawn/ring/pingpong — fairness intact); a standalone `http` "0"
scare turned out to be the missing harness-managed server (0 on all three engines,
incl. the tree-walker my change can't touch). Suite 777/777; 10-row correctness
gauntlet OK; fmt clean.

## 2026-07-15 — Two profiled cuts: non-exact int `/` inlines (mandelbrot −17%); spilled vectors read through a cached pointer (nbody −9%)

Profiling the "boxed array math" class before any storage redesign paid off twice —
neither finding was the storage:

**mandelbrot's 582,120 `prim2_fallback`s** were `(/ px n)`: non-exact int÷int
deferred to a full dispatch per pixel (540² pixels × 2 divisions — the count matched
exactly). `prim_apply`'s `Div` now yields the float `a as f64 / b as f64` inline —
exactly `prim_div`'s int arm, `i64::MIN / -1` included; only ÷0 still defers for the
native's exact error. mandelbrot 0.24 → **0.20 s**; semantics verified identical on
both engines (incl. MIN/−1 and the ÷0 error).

**The general vector-read FFI (~20 ns/element).** `VecStore::Spill` now carries a
cached `(ptr, len)` header (`#[repr(u8)]`-pinned at disc 1 / ptr @8 / len @16,
layout-tested): sound because a spilled buffer never moves — contents are immutable,
and slab growth / GC copies move the three-word struct, not the heap buffer it points
to; `Clone` is hand-implemented to re-derive the pointer (a derived clone would alias
the original buffer). The JIT then reads LOCAL vectors fully inline on BOTH index
shapes: the constant-index path's heap-backed branch (nbody's 7-element body fields —
formerly the `brood_rt_vector_ref` FFI per read) and a new dynamic-index inline
(tag/region/int-index checks → slab slot → inline-or-spill element read), with the
FFI kept as the exact-semantics fallback for RUNTIME/PRELUDE regions, non-vectors,
and out-of-range. nbody 0.54 → **0.49 s**.

**The honest boundary:** matmul stays FFI-bound — its `def`'d matrices are
RUNTIME-region, and that slab is `boxcar`-backed (bucket-indexed; no flat base to
inline against without reaching into boxcar internals). Inlining RUNTIME vector reads
needs a JIT-visible RUNTIME vector arena — noted as the follow-on. Verified:
GC-stress + GC_VERIFY + JIT_VERIFY clean on the vector-heavy rows (the load-bearing
check for a `VecStore` layout change); 21-row gauntlet 3-engine bit-identical; suite
777/777; fmt clean.

## 2026-07-15 — Refuted: deferred frame stores for register-carried JIT loops (≤2%, reverted)

Hypothesis (from the prior session's profiling note): a register-carried self-tail
loop's per-iteration back-edge stores of the carried scalar slots to `roots` cost
~8 ns/iter, and exist only to give a rare deopt/preempt the frame's resume state —
so eliding them (the live value already flows in a Cranelift `Variable`) and
reconstructing `roots` on the exit paths should be a broad win for
`loop`/`collatz`/`mandelbrot`/`nbody`.

Built it: back-edge skips the carried-slot stores; a `deopt` shim spills the
*current* iteration's carried registers (header values) before the raw deopt, a
`poll_deopt` shim + the `preempt` block spill the *next* iteration's (post
back-edge `def_var`); the carry update became a register-level parallel assignment.

**Measured ≈2% and reverted.** A/B (worktree on `dc66500`): loop 2.53 → 2.48 s at
N=300M (0.17 ns/iter, not 8), collatz/mandelbrot ~1 tick, nbody/nqueens flat. The
stores were never the bottleneck — they target hot L1 frame words and the store
buffer + OoO fully hide them behind the loop's arithmetic. Not worth the cost: the
change introduces a **"`roots` is stale for a carried slot"** invariant into the
hottest, most delicate JIT code, and *every* site that reads a carried slot from
`roots` becomes a silent miscompile. Three surfaced during the build, each a
too-small final result: `read_words` (returning/aliasing a carried slot),
`store_op`→`copy_value` (the `exit_done` roots→roots copy of the accumulator — the
big one), and the parallel-assignment ordering. `load_slot_int`/`as_int`/`as_f64`
already had the carry fast path; the point is that keeping `roots` authoritative
every iteration is the safer invariant and costs ~nothing. **Don't re-chase frame
stores** — profile a genuinely allocation- or FFI-bound benchmark instead.

## 2026-07-15 — Regex's dead `(:use editor/buffer)`: 578 → ~301 ms wall, RSS 182 → 65 MB (one line)

Right after the frame-stores dead end, took its own advice — *measure the benchmark,
don't assume* — on `regex`, the sole surviving last-of-seven (578 ms). Bisected the
cost and it was **none of** the matcher, the per-call compile (cached — a string-keyed
`table-get`, 0.03 s/20k), or the codes conversion. It was **module load**: `require
'regex` alone cost **0.29 s** vs `require 'json` ~0. The regex module header carried
`(:use editor/buffer)` — pulling in the 862-line editor/buffer module (which loads in
~0.33 s all by itself) — and **regex references nothing from it** (checked all 89 of
buffer's public defns against the whole file: zero hits). A pure dead dependency,
paid once per process at startup. Deleted the one line.

Result: `require 'regex` 0.32 → **0.03 s**; the benchmark **wall 0.578 → 0.301 s**,
**peak RSS 182 → 65 MB**. **Correction (measured, not assumed):** this does NOT move
regex off 7/7. The suite ranks by **compute = wall − startup**, and clojure/elixir boot
a JVM/BEAM (330/190 ms) that gets subtracted — their regex *wall* is 477/197 ms but
their *compute* is only 144/10 ms. Brood's regex compute fell **547 → 270 ms (−50 %)**
— the whole `editor/buffer` load was in compute (the `startup` row is a bare program,
so a `require`d module's load counts as work) — but clojure's native regex compute
(144 ms) is still faster, so **regex stays 7/7 by compute** (the gap closed ~3.8× → 1.9×,
and any program that `require`s regex now boots ~290 ms sooner). Verified: regex suite
14/14; 3-engine (JIT / NO_JIT / tree-walk) bit-identical; the checked-but-**kept** deps
(sexp genuinely uses `buffer-text`/`goto-char`/… ; leave it).
Follow-on noted, not chased: **editor/buffer itself loads in 0.33 s for 862 lines** —
json's 396 load in ~0, so large-module load time is superlinear/pathological and would
speed up real editor + `nest` startup broadly. The general lesson: a benchmark's wall
time includes process startup, and a stray `:use` taxes *every* run — measure load vs
work before optimizing the work.

## 2026-07-16 — match/receive lowering was EXPONENTIAL in arm count (editor/buffer load 297 → 7 ms); require's stale in-flight marker (5 s stall on a failed load)

Chased the "editor/buffer loads 0.33 s for 862 lines" follow-on. It was neither
module size nor the loader: a synthetic module of **800 trivial defns loads in
12 ms** (perfectly linear), and per-form timing (`read-all` + `eval` each top-level
form) fingered **one form** — `buffer--serve`, the 15-arm `receive` loop — at
206 ms of the 297 and ~130 MB of RSS.

**Root cause: `match-build-from` compiled each clause with the full rest-of-clauses
code as its fail continuation, and the pattern compilers splice `fail` at EVERY
failure point** — a vector pattern pastes it once per element test, so the tree
~doubles per arm. Measured: 8-arm receive 15 ms, 12 arms 331 ms, **16 arms 7.0 s**
(plain `match` identical). Every multi-arm `match`/`receive`/multi-clause `fn` in
any program paid this at compile/load time.

**Fix (std/prelude.blsp, `match-build-clause`):** compile the clause against a
gensym'd thunk call `(k)` instead, count its uses in the compiled tree, then:
0 uses → drop rest-code (irrefutable pattern, rest is dead); 1 use **or small
rest-code** (≤64 pair nodes, early-exit budget probe) → splice it in place —
bounded duplication, so the hot 2–4-arm dispatch (pingpong's receive) generates
**identical code to before**, zero runtime cost; otherwise bind once as
`(let (k (fn () rest-code)) …)`. `(k)` sits exactly where `fail` did — tail
position — so TCO through a chain of fail thunks holds (verified: 200k iterations
falling through 11 arms, O(1) stack). The small-inline threshold matters: the first
cut thunked unconditionally and cost pingpong +7% (a thunk alloc per received
message); with it, old-vs-new benchmarks are flat (pingpong/ring/fib/collatz/json/
nqueens/bintree/sort/regex/errors-deep, interleaved best-of-N).

Results: `match` 16 clauses 3441 → **2 ms**; 16-arm receive 6981 → 2 ms;
`require 'editor/buffer` 297 → **7 ms** (whole process 0.32 s / 153 MB →
0.03 s / 27 MB); `sexp` (uses buffer) 294 → **11 ms**. Anything loading the
editor stack — brood-edit boot, `nest` tooling over sexp — gets ~290 ms back.
Verified: suite 777/777; `nest check` zero warnings; the match-semantics gauntlet
(guards, list/vector/map/string/int patterns, `& rest`, `:or` defaults, no-match
throw, cross-process receive) bit-identical on all 3 engines; GC-stress clean.

**Bug #2, found via the same diagnosis:** a `require` whose load THROWS (module
not found, or an error inside its source) left its `*features-loading*` marker
behind — a later require of that key in another process stalled the full
`require--await` window (**~5 s** of 5 ms ticks) before taking the load over, and
a same-process retry silently "succeeded" via the circular-require check. A failed
`(require 'nope)` measured 5.4 s wall / 0.3 s CPU. `require-one` now clears the
marker on the throw path too (catch → dissoc → rethrow); a failed require errors
instantly and a retry attempts the load afresh.

Lesson: "superlinear module load" was really "one pathological form" — time the
forms, not the file. And the old benchmark memory's "regex residual is module
load" chain ends here: the load cost itself was the match expansion.

## 2026-07-16 — JIT: closure arms through the call-profitability gate + deopt feedback (nqueens −31%, nbody −28%, pipeline −14%)

Set out to profile the presumed "allocation rate" frontier on bintree/nqueens.
The profile refuted it on both rows:

- **bintree is call-protocol-bound**, not alloc-bound: `jit_run_fast_link` 27% +
  `brood_rt_fast_frame` 10% + `push_n` 5% + frame memmove 14%, vs ~7% in
  `make_vector2` (the actual allocation). That's the known call-convention
  frontier ("the big one") — left alone this session.
- **nqueens never ran its hot code natively at all**: 348k per-element calls of
  the `reduce` step closure, every one through the full `vm_apply` →
  `vm_run_bc` → `push_frame` trampoline (≈45% of the row), `jit_link_done` = 0.
  `hof_apply_native` exists precisely for this and never fired — the step arm
  never compiled, refused by the static profitability gate (jit_lower_arm's
  "call-mediated boxed work does not win natively": ≥1 non-tail call, no vector
  op, no self-loop → bail).

Two changes, shaped by three measured dead ends (recorded so they aren't
re-walked):

1. **The static gate now applies to top-level defns only; closure arms are
   exempt.** A closure arm (the HOF step shape) going native is what lets
   `hof_apply_native` skip the trampoline per element: nqueens wall 0.16 →
   **0.11 s** (compute ~130 → ~80 ms — past Ruby's 123 ms, **6/7 → 5/7**),
   pipeline 0.07 → 0.06, a closure-reduce microbench −62%. The dead ends: (a)
   removing the gate outright also won nqueens but regressed nbody +11% —
   native `advance` linked into `advance-body`, which deopts, re-runs on the
   VM, re-tiers, and ran native TWICE per call; (b) refining the gate on the
   float-slot profile didn't protect nbody (the thrasher's floats live in
   let-binder slots, which snapshot as nil at enqueue); (c) admitting defns
   with a ≥4-work floor still regressed `spawn` 0.08 → 0.3–1.3 s erratic
   (145k context switches, 20× task-clock) — the newly-admitted hot defns were
   the PRELUDE's own compile machinery (`match-count-sym`, `match-splice-fail`,
   `seq`, `fold`), which every spawned process's compile path runs under
   10k-process fan-out. Hence the final scoping: defns keep the old gate
   verbatim; closures are governed dynamically by —

2. **Deopt feedback** (`deopt_watch` on `CompiledArm`): every non-loop arm with
   ≥1 non-tail call — closure or defn, vector ops included — counts
   **consecutive** type-deopts (a success resets; `SelfCall` loop arms are
   exempt, their deopt follows productive native iterations); 16 in a row
   store `BAILED`, so a persistently-thrashing arm self-heals onto the VM.
   This caught a bug nobody had seen: **nbody's `advance-body` has been
   deopting on ~100% of its 248k activations in the baseline too**
   (`jit_deopt ≈ jit_native`, paying native entry + deopt + a full VM re-run
   per call, invisible because the row still "worked"). With feedback it bails
   at exactly deopt #16: nbody 0.47 → **0.34 s** (−28%, compute ~440 → ~310 ms,
   within a hair of Ruby's 303) — better than the baseline ever was, with the
   gate never having admitted anything new on that row. `BROOD_DEOPT_TRACE=1`
   (perf-stats builds) prints each deopt's arm for exactly this diagnosis.

Counters after: nqueens `vm_apply` 348k → 41k, `jit_link_done` 0 → 684k,
deopts 0; nbody `jit_deopt` 248k → 16, `jit_link_rerun` 0. Verified: suite
777/777; `nest check` zero warnings; 3-engine bit-identical on 8 rows;
GC_STRESS + GC_VERIFY + JIT_VERIFY clean; interleaved A/B flat on
spawn/pingpong/ring/base64/fib/collatz/json/sort/sieve/loop/matmul/bintree.

Standings: nqueens moves to 5/7; nbody's 6/7 gap to Ruby closes 140 → ~7 ms;
the two open frontiers are unchanged in kind — bintree/regex hang on the call
convention, json/base64 on native-library rivals.

## 2026-07-16 — sieve deep-dive: lock-free dense Table + resume-tier fix (sieve −33%, loop −75%)

Chased sieve's 6/7 (~120 ms compute; the rivals all use a lock-free mutable
byte array — python `bytearray`, elixir `:atomics`, .NET `bool[]`). Three
landings and one instructive revert:

**1. The dense Table path is now lock-free** (`table.rs` rewrite). A dense op
was `lookup → Mutex lock → Vec index → unlock`; perf-annotate put the mutex at
~half the ~27 ns/op and the 16-byte `DenseVal` enum's second cache line in the
rest. Now: slots are ONE anonymous `mmap` region of `2^23` atomic words
(64 MB **virtual**, pages committed on first touch — sieve RSS 60 → 32 MB, and
the old sparsity guard is gone: a far-out key costs one 4 KB page, not a
resize), values pack into tagged u64s (EMPTY/NIL/TRUE/FALSE/int«3; ints beyond
±2^60 migrate), and every `put`/`get`/`has?` is one atomic op + a flag load.
`table-incr` is a lock-free CAS loop — stronger than the old locked RMW.
Migration to the hashed map keeps the mutex and captures each non-EMPTY slot
with `swap(MOVED)`; per-slot atomic total order + a post-op flag re-check make
racing lock-free ops exact (put/delete redo idempotently; incr resolves its one
ambiguous interleaving under the migration lock; full protocol on `Store`).
An 8-process race of 80 k puts + 80 k incrs across a mid-run migration loses
nothing. First cut used `boxcar::Vec` — its per-entry init-flag byte was 37% of
`put` (a second dependent cache miss per op); the flat region removed it.

**2. Resumed native loops no longer interpret 256 iterations per timeslice**
(`try_jit = fresh || cur_ip == 0`). The tier hook only fired on *fresh*
activations, so a preempted JIT'd self-tail loop resumed on the interpreter
and re-tiered at the next 256th back-edge. sieve's `mark` preempted ~1030
times → ~260 k interpreted iterations (~20% of the row); `loop` was worse:
0.28 → **0.07 s** (−75%) from this one line. An ip-0 resume is always "run the
whole arm against the current slots" — a preempted self-tail frame parks reset
(ip 0, carried slots live), which is exactly its native re-entry; a
mid-`receive` resume rewinds to a nonzero ip and still never tiers.

**3. A self-tail loop stuck QUEUED sync-compiles** after ~2k back-edges
(`jit_compile_now`, called from the driver's tier hook): a bounded ~ms block
on the loop's own thread instead of an unbounded interpreted tail while the
cold-start background compile crawls the queue. The first cut called the
compiler from *inside* `exec_chunk`'s dispatch loop and wrecked its codegen
(+45% branch misses, json −10%) — moved to `vm_run_bc`, keeping only a bare
exit-condition in the loop. Also: the background compiler now skips arms a
sync compile already resolved, and `BROOD_COMPILE_TRACE=1` (perf-stats builds)
prints per-arm compile latency.

**Reverted: disabling cranelift's `enable_verifier` in release.** The extra
flag made `JITBuilder::with_flags` fall back to DEFAULT flags — silently
dropping `opt_level=speed` for every compiled arm (json −10% was the tell;
found by per-file bisect against the previous commit). Compile latency was
already fine (380 µs/arm measured); nothing was lost by reverting.

Numbers (interleaved best-of-3 vs `bf41695`): sieve 0.15 → **0.10 s**
(compute ~120 → ~70 ms — past python 123 and ruby 85, **6/7 → 4/7**; elixir's
53 ms is next), loop 0.28 → **0.07 s**, wordcount 0.08 → 0.06, everything else
flat (spawn reads +1 tick on identical instruction counts — layout wobble).
Verified: suite 777/777; `nest check` zero warnings; 3-engine bit-identical on
10 rows; GC_STRESS + GC_VERIFY + JIT_VERIFY clean; the concurrent-table race
test exact. `Heap::hash_int` is the one heap API addition (int structural hash,
heap-free, for migration).

## 2026-07-16 — JIT inlines dense table ops (sieve 0.10 → 0.06 s, 4/7 → 3/7)

The wrapper chain around one xchg — `brood_rt_table_put` arg marshalling 12%,
`table::put` prologue, `lookup` 4%, `check_key` 3% — was ~70% of what remained
of sieve. The vector LICM (`brood_rt_vector_base`, matmul) already shows the
shape, and the lock-free rewrite made it LEGAL for tables: the dense slot
region is one process-lifetime mmap that never moves (not a heap object —
stable across GC and compaction), and migration/drop are observable per-op via
the MOVED sentinel + dense flag.

So: `Op::HoistedTable` — when a hoisted scalar global resolves (at arm entry,
via the new `brood_rt_table_dense_base`) to a dense table, its slot base + flag
address ride with the value words. A `(table-put g k v)` in the loop is then
ONE inline atomic xchg (key bounds-checked, value tag-encoded inline —
Int/Bool/Nil, mirroring `slot_enc`) + the protocol's post-op flag check; a
`(table-has? g k)` is one atomic load. **Every guard failure — null base
(hashed/non-table global), non-int or out-of-range key, unencodable value,
MOVED, flag flipped — branches to the per-op FFI block, never a deopt**, so an
odd shape can't thrash the arm (the advance-body lesson) and a hashed table
just keeps its FFI cost. A rebind of the global itself is the existing
back-edge epoch guard's job. Inline ops skip the exact-count/watermark upkeep,
so handing out a base latches `jit_shared` on the store: `table-count` tallies
by scan and migration/snapshot/drop scan the full region (reads of untouched
zero pages commit nothing).

sieve 0.10 → **0.06 s** (compute ~30 ms — past Elixir's 53, **4/7 → 3/7**;
ahead only node 6 ms / .NET 3 ms, both on native arrays). Verified: suite
777/777; 3-engine bit-identical; GC_STRESS + GC_VERIFY + JIT_VERIFY clean;
`nest check` zero warnings; a 5×-repeated race of a JIT'd 200k-put loop against
a mid-run dense→hashed migration (string key from another process) loses
nothing (`bad: 0`, exact count, every value intact); the full sweep flat
everywhere else (regex/json/persistent-map — the other table users — unchanged).

## 2026-07-16 — the stress suite (`make stress`) + KNOWN BUG: JIT deopt re-run duplicates side effects

Built the occasional big stress run under `stress/` (deliberately outside
`tests/` discovery — `make stress`): a property-based differential of the
lock-free Table against an immutable-map model (seeded LCG op sequences over
every representation boundary: dense/hashed keys, ±2^60 tagged-int edges,
stored-nil-vs-absent, migration mid-sequence), multi-process races (8 writers
across a mid-run migration; concurrent `table-incr` exactness; drop-under-fire
conservation; snapshots under writers), VM/JIT loop torture (16×2M-iteration
preempted native loops, collatz checksums, a float-crossing deopt loop), match
lowering vs a hand-written cond oracle (60k random values + 300k-deep
fail-thunk TCO), and a **cross-language differential** (an identical LCG op
sequence driven against a python dict; digests must match — they do). Runner
sweeps JIT / no-JIT / GC-stress; `xfail_*` files are known-bug repros reported
separately (an unexpected pass = promote to `tests/`).

**It caught a real one immediately — a long-latent JIT soundness hole.**
`stress/xfail_deopt_rerun_test.blsp`: a JIT'd self-tail loop whose body does a
side effect (`table-incr`) and then destructures a **call-result vector**
duplicates the effect once per 256-edge tier boundary. Chain: the destructure
emits a deopt-capable instruction that fires on EVERY native entry of the arm;
an outcome-1 deopt "re-runs the arm on the VM with the frame intact" — from
ip 0 — so everything before the deopt point, side effects included,
**executes twice**. Diagnosed with an execution-counter table (each op index
must run exactly once): doubles at exact 256-multiples, immune to
`BROOD_REDUCTIONS`, gone under `BROOD_NO_JIT=1`. Bisect: reproduces on
**ed502ba (pre-session)** at 60k iterations — pre-existing; today's faster
tiering (tier-on-resume, sync compile) only surfaces it at smaller N. Control
variants that do NOT reproduce: manual `(nth v 0)` instead of a destructure,
and destructuring a locally-built vector literal (escape analysis elides it).

Scope: needs a lowered arm + a non-idempotent effect (`table-incr`, `send`,
io — NOT `table-put`/`delete`, which re-apply idempotently) + a later deopt in
the same activation. `receive` loops are safe (their per-arm closures carry
`MakeClosure`, which never lowers). Deopt feedback doesn't help — self-call
arms are exempt by design, and the first 16 duplications would land anyway.

Fix directions (next session, needs design + benchmarks — NOT a quick patch):
the real fix is **deopt-site metadata** (resume interpretation at the deopting
instruction — statement boundaries have an empty operand stack, so per-site
resume-ip + slot-state suffices); the interim conservative option is bailing
lowering when a potentially-effectful instruction precedes a deopt-capable one
(cost unknown — it would hit fib-shaped pure arms unless purity is tracked).
Until then: table_model stress runs idempotent ops only, and the incr coverage
lives in the xfail. ALSO worth fixing regardless: the destructure-of-call-result
emits a deopt on every entry — finding and lowering that instruction properly
removes the whole "every-entry deopt" class (such arms gain nothing from the
JIT anyway and currently pay native entry + deopt + VM re-run per call).

## 2026-07-16 — FIXED: JIT deopts resume at effect-safe checkpoints (the deopt-rerun bug)

The morning's KNOWN BUG is closed the real way — **deopt-resume checkpoints**
— rather than by restricting what lowers:

- `compile_arm` runs a tiny static pass (`jit_ckpt_depth`: an abstract
  interpreter over the chunk propagating operand-stack depth; both branch
  edges, merge-consistent) and, for any arm with a non-tail call, reserves a
  **checkpoint area** above the spill slots: one journal slot (packed
  `(resume_ip << 16) | depth` as a plain `Value::Int`) plus room for the
  deepest post-call operand stack (`CompiledArm::ckpt_slot`).
- The lowering journals after **every completed non-tail call**: each abstract
  operand (only GC-safe shapes exist there — unboxed scalars, frame slots, the
  fresh call result) is stored into the checkpoint slots, then the packed
  ip/depth. Entry and every self-tail back-edge reset the journal to 0; the
  inlined-upgrade lowering resets at entry but never journals (its chunk ips
  aren't the interpreter's).
- On an outcome-1 deopt every consumer (vm_run_bc's tier hook, the dispatch
  fast path, `jit_run_fast_link`, `jit_dispatch_call`, `hof_apply_native`)
  reads the journal: ip > 0 ⇒ push the journaled operands and **resume the VM
  at the checkpoint** — the frame is intact, exactly the shape of a frame
  suspended at a `Call` (the three FFI-side consumers drive it via a synthetic
  single-frame `vm_run_bc` resume, `vm_resume_deopt`). ip = 0 ⇒ the legacy
  from-ip-0 re-run, which is now effect-free by construction: everything the
  boxed subset executes besides calls is pure or idempotent (the inline table
  put re-applies; allocs re-allocate). Effects therefore execute exactly once,
  always. `BROOD_NO_DEOPT_RESUME=1` is the A/B lever (per-arm, at chunk time).
- Frame slots make the journal GC-traced and nesting-safe for free (a callee's
  own journal lives in its own frame) — no heap fields, no FFI on the hot path.
  Cost: a few stores per completed call; the full benchmark A/B is flat.

Two landmines hit while landing it: the depth pass must not count a
free-global-head call's callee as a staged operand (the IC resolves it — the
pass returned None for every real arm until fixed), and the **spill area must
be measured from the checkpoint base**, not `nslots` (spills and journal
otherwise collide — caught by `nested_ifs_and_multiple_args_under_jit`'s
3-calls-plus-adds shape, and only there: a reminder that the Rust JIT tests
earn their keep).

Verified: the xfail repro flips to green and is **promoted to
`tests/jit_deopt_effects_test.blsp`**; the model stress regains non-idempotent
`table-incr` coverage (8/8 ×5 incl. GC-stress); suite 777/777; `make stress`
fully green (no xfails remain); 3-engine bit-identical ×8 rows; `nest check`
clean; benchmarks flat vs the `BROOD_NO_DEOPT_RESUME` baseline.

## 2026-07-16 — stress suite grows external-style batteries (R7RS- and Clojure-inspired)

`make stress` now also runs two adapted correctness batteries: 
**core_semantics** (the shapes Chibi's R7RS suite exercises, asserted against
Brood's documented semantics — i64 edges + exact bigint promotion, truncating
`quot`/`rem`, exact-vs-float `/`, type-strict `=` with cross-type ordering,
IEEE float identities, unicode char-indexed strings, 1M-deep self- and
mutual-tail calls, closure/scoping laws, structured throw/catch through 50k
frames) and **collections** (Clojure-style invariants: CHAMP assoc/dissoc
round-trips at 20k scale with immutability asserts, exact counts under churn,
merge bias, mixed-type keys, vector index ops, and the sequence laws —
map composition, filter/remove partition, fold associativity, take/drop
partition, reverse involution, sort idempotence, 10k-map deep equality).
39 new cases, swept across engines + GC-stress by the runner. Two
actual-semantics findings while writing them: `nth` out-of-range on a vector
is nil (not an error; `get` takes its default), and `docs/brood-for-claude.md`
line ~273 mentions `conj` for vector append but no `conj` exists (`append`
returns a LIST even from vector inputs) — doc or design gap, left for a
deliberate decision rather than a drive-by edit.

## 2026-07-16 — conj lands; the stress kit gains a program fuzzer + chaos preemption

- **`conj`** (prelude, pure Brood): the batteries caught `language.md` AND
  `brood-for-claude.md` both promising it while nothing defined it. Clojure
  semantics: append to a vector, prepend to a list (`nil` = empty list),
  `assoc` a `[k v]` / merge a map into a map; variadic via `fold`. Tests in
  `tests/vectors_test.blsp`.
- **Random-program differential fuzzer** (`stress/fuzz_programs.py`,
  Fuzzilli-lite): seeded, deterministic, terminating programs from a small
  grammar — pure helpers over i64/bigint-edge ints and floats (if/let
  nesting, guarded division, vector build+index), an effectful self-tail
  driver (table-put + table-incr — non-idempotent by design; this is the
  shape that caught the deopt-rerun bug) and a digest print. Each program
  runs under four configs — jit, no-jit, gc-stress, and **chaos-preempt**
  (`BROOD_REDUCTIONS=97`, a tiny prime budget forcing preempt/resume storms)
  — and any stdout difference (agreed errors included) fails the run, with
  divergent programs kept in `stress/fuzz_out/`. `make stress` runs 25 seeds;
  crank `--seeds` for long hunts.
- Chaos-preempt passes added for the table-model and vm-loops batteries.
- Noted for a deliberate look: `BROOD_VM=0` no longer selects the tree-walker
  for top-level program code (observed VM counters under it while debugging —
  likely a consequence of ADR-135's top-level-as-capture-process; either the
  flag or CLAUDE.md's description should be updated).

## 2026-07-16 — BROOD_VM=0 honored again at top level; the fuzzer gets its third oracle

ADR-135 (top level as a capture-mode process) silently swallowed `BROOD_VM=0`
for `brood file.blsp`: `run_program_body` always drove forms through the VM, so
the "tree-walker" leg of every engine-differential comparison was really the VM
(discovered while building the fuzzer — its "TW" runs showed VM counters).
`run_program_body` now honors the flag: each expanded form runs synchronously
on `eval::eval` (`def` is its special form; a top-level `receive` blocks the
worker — the documented tree-walker behavior; it is a debug engine, not the
production path). Verified: zero VM work-attribution counters under
`BROOD_VM=0`, results identical. The fuzzer gains `tree-walk` as a fifth
config — a genuinely independent oracle again — and 30 fresh seeds agree
across all five. Suite 777/777; `make stress` fully green.

## 2026-07-16 — float printing goes shortest-round-trip; a reader/printer round-trip battery; an honest false alarm

The new `stress/reader_roundtrip_test.blsp` property (`read-string ∘ pr-str =
identity` over seeded nested values) drove two things:

- **Float printing now uses Rust's `{:?}`** (shortest round-trip rendering):
  `1e300` prints as `1e300` instead of a 301-character decimal expansion, and
  `5e-324` likewise — both still read back exactly. Normal floats are
  unchanged (`1.0`, `0.1`, `-0.0`).
- **A false alarm, kept for the lesson:** a probe showed `pr-str` emitting
  `inf`/`nan` and I "fixed" it to Clojure's `##inf` spellings, breaking
  `tests/float_roundtrip_test.blsp` — which documents that the READER already
  parses bare `inf`/`-inf`/`nan` as float literals; the old design was
  symmetric all along. Reverted to the existing contract. The lesson: a
  round-trip probe must test BOTH directions before concluding anything —
  print-side inspection alone said "bug" where there was none. (The 777 suite
  catching the misfix within minutes is the system working.)

Suite 777/777; `make stress` at 22 green runs (round-trip battery included,
engines × GC-stress).

## 2026-07-16 — stress kit round 3: concurrency fuzzing, formatter properties, checker soundness

- **Concurrency in the program fuzzer**: generated programs now optionally fan
  out 4–16 workers running a generated pure helper plus commutative shared-table
  ops (`table-incr` on one key, disjoint-range puts), fanning results in via
  `receive`. The digest is deterministic even though scheduling is not — any
  divergence is a real concurrency bug, never schedule noise. (One generator
  lesson: a worker crash turns the fan-in's timeout into an all-config hang
  that *looks* like agreement — workers now avoid the float helper whose
  result the masking `bit-and` rejects, and the timeout is 10s.)
- **Formatter properties over the whole repo** (`stress/formatter_test.blsp`):
  the strong pair — SEMANTIC PRESERVATION (`read-all (format-source src)`
  renders identically to `read-all src`) and idempotency — verified over every
  `.blsp` in `std/**`, `tests/`, `stress/`, plus a tricky-syntax corpus
  (quasiquote/splicing, escape-laden strings, bytes literals, comments in every
  position, non-finite floats, guarded match patterns). One property lesson:
  compare via `pr-str`, not `=` — a `nan` literal in the source never equals
  itself, producing a phantom diff on byte-identical parses (hit twice today;
  now written down). The formatter itself came out clean.
- **Checker soundness harness** ("sound in all aspects, not necessarily
  complete"): (1) the fuzzer now runs `brood --check` on every seed whose
  program runs cleanly on all engines and fails on any TYPE warning (style
  lints — unused binder, non-tail recursion — are legitimate on generated code
  and excluded); (2) `stress/check_corpus/` holds seven hand-built valid
  programs in the historically false-positive-prone shapes — int/float
  widening, global redefinition across types (the reload contract), match
  branches joining different types, HOF/closure flows, sequence polymorphism,
  dynamic table gets, bigint promotion — each must run clean AND check clean.
  All pass; 100 fresh fuzz seeds type-warning-free.

`make stress` is now 31 green runs. Suite 777/777.

## 2026-07-16 — TSAN clean, loom model-check, fuzzer auto-shrink

- **ThreadSanitizer** (`make tsan`: nightly + build-std + a new `system-alloc`
  feature): a new 8-thread table hammer (`tests/table_tsan.rs` — puts/incrs/
  gets/deletes racing a mid-run migration and a drop) plus the existing
  concurrency/preemption/live-migration/GC tests, all under TSAN. First run
  screamed 195 data races — every one a **mimalloc artifact**: its
  un-instrumented C internals hide the free→alloc happens-before, so every
  cross-thread block reuse reports as a phantom race. With allocations routed
  through the (interceptable) system allocator: **zero reports across all 15
  tests**. The scheduler, shared code region, promote/spawn, JIT compile, and
  the lock-free table protocol are TSAN-clean.
- **Loom model-check** (`make loom`, `tests/loom_table_protocol.rs`): the
  dense-table migration protocol as a faithful miniature (the real slots live
  in an mmap loom can't instrument), exhaustively interleaved — disjoint and
  same-key put races, exact increments, and reader coherence across a
  migration all hold. Big caveat discovered en route: **loom 0.7 does not
  model the C11 SC total order for plain SeqCst accesses** — a classic
  store-buffering litmus FAILS under it (kept in the file as evidence). The
  model therefore expresses its store→load orderings as explicit SeqCst
  fences (which loom handles); the real code needs no fences — its SeqCst
  RMWs/stores/loads carry the SC order per C11. Four earlier "lost write"
  loom failures were exactly this artifact, chased to ground via the litmus
  before touching a line of table.rs.
- **Auto-shrink**: any divergent fuzz seed is now minimized automatically —
  s-expression-level delta debugging (drop top-level forms, replace subtrees
  with `0`, halve int literals, to a fixpoint under the still-diverges
  oracle). Validated end-to-end with a synthetic oracle: a 697-byte generated
  program reduced to the true 25-byte minimum in 124 oracle runs. The `.min`
  file lands next to the kept seed in `stress/fuzz_out/`.

Suite is now 779 (the TSAN hammer runs as a plain stress test too); all green.

## 2026-07-16 — Auto-shrink pays off: JIT sibling-`let` slot-reuse miscompile

The fuzzer's new auto-shrink earned its keep on its first real catch. Seed
20108 diverged: the JIT computed `digest 268435109 18 8` where every other
engine (VM, tree-walker, GC-stress) agreed on `0 2102 8`. Delta-debugging plus
a hand reduction cut it to a one-liner:

```
(defn f (p) (- (let (a 300) a) (let (b 5) b)))   ; warmed past tier → 0, not 295
```

**Root cause** (`jit_lower.rs`, the general bytecode→CLIF lowerer). `Inst::Local`
pushes a **lazy** `Op::Slot(i)` — a "read frame slot `i` at the consumer" token —
onto the lowerer's operand stack. The bytecode compiler **reuses one slot index
across sibling `let` scopes** (`a` dies before `b` is bound, so both get slot 1).
That is sound for the VM, whose operand stack holds *materialised values*: the
left operand's 300 is already on the stack when `b`'s `SetLocal` overwrites the
slot. But the JIT's lazy `Op::Slot(1)` for the left operand re-reads slot 1 at
the subtract — by then holding `b`'s 5 — so `(- (let a…) (let b…))` lowered to
`(- 5 5) = 0`. The IR dump showed it plainly: two stores to the *same*
`(base+1)*STRIDE` address, then both reads loading it back. Only the general
lowerer was affected; the SSA i64 fast path (`lower_i64_value`, distinct
`def_var` per binder) was already correct, which is why it took a two-`let`,
two-operand shape to surface.

**Fix**: at `SetLocal(i)`, before the overwrite, materialise every still-pending
`Op::Slot(i)` on the operand stack to the slot's *current* (pre-store) value —
reconstructing its exact type from the slot caches (`Op::Float` via bitcast for
a float slot, `Op::Bool` for a bool slot, else `Op::Handle` of the three words)
so consumers behave identically to the lazy read they replace. Common case
(SetLocal to a fresh slot, nothing pending) is just an O(depth) scan that finds
nothing.

Verified: min repro and seed 20108 now agree across all engines; float/bool/int
reuse shapes all bit-identical VM↔JIT; new regression `tests/jit_let_slot_reuse_test.blsp`
(4 tests); 800 fresh fuzz seeds clean; full suite green (2754 in-language).

## 2026-07-16 — Coverage-guided fuzzing finds a second bug: VM error-format divergence

Pushed the differential fuzzer harder. Two structural upgrades, one real bug.

**Grammar expansion + coverage guidance.** `llvm-cov` over a fuzzer sample showed
the generated programs hit only **64%** of `jit_lower.rs` — and the 21 dark
functions were almost entirely the **i64 fast-path lowerer** (`jit_lower_arm_inner`
& gates). The table/closure/match-heavy programs are too complex to qualify for
that specialised SSA path, so a whole second JIT engine was unfuzzed. Added a
restricted pure-i64 expression generator + standalone pure self-recursive numeric
fns (int accumulator, fib-like non-tail double recursion, float recursion), which
lift `jit_lower_arm_inner` from 0% to 23–98%. Also added maps-as-values, try/catch,
nested closures, process trees (spawn+monitor+selective-receive), and a
slot/operand-torture helper (sibling `let`s in operand positions + shadowing — the
neighbourhood of the sibling-let slot-reuse bug).

**Harness hardening: re-confirm before reporting.** A concurrent instrumented build
starved fuzzer subprocesses and produced two "divergences" that vanished on re-run
(220×/120× identical). The fuzzer now re-runs a flagged seed 3× and only reports if
it STILL diverges — a real engine-diff reproduces, an OS-contention artifact
converges. Makes the sweep trustworthy under load.

**The bug (fuzzer seed 70002, pure-recursive shape).** The `brood` file runner
rendered a top-level runtime error as `file: LINE:COL: msg` under the VM/JIT but the
canonical `file:LINE:COL: msg` under the tree-walker — a stray space, and the VM's
form is NOT the `file:line:col` shape editors/LSP parse. Root cause:
`ProgramState::crash` called `located()` *before* the file was attached (yielding
`LINE:COL: msg`), then string-prepended `file: ` with a space. Fixed to attach the
file to the error's own field (`or_file`) so `located()` renders the canonical
prefix once, identical to the tree-walker. CI regression:
`crates/cli/tests/error_format_parity.rs` (type/unbound/arity/thrown errors, all
three engines byte-identical). Also tightened the generator so the driver never
bit-ands a float-helper result (gratuitous type errors; floats covered by the pure
`flt` recursion). Full suite 779 green; error-parity 4/4.

## 2026-07-16 — ASAN pass: kernel is memory-clean; i64-path fuzz edges

**AddressSanitizer** (`make asan`: nightly + `-Zbuild-std` + `system-alloc` so
ASAN intercepts allocations instead of mimalloc's un-instrumented arena) over the
whole `-p brood` test surface — every integration test, plus the entire
in-language suite via `suite.rs` (45 s under ASAN). **Zero AddressSanitizer /
UBSan reports.** The unsafe substrate — the 2^23-slot mmap dense-table + its
lock-free migration, JIT codegen buffers, the moving GC, the scheduler, promote/
spawn — is memory-clean, not just logically-clean (the GC tripwires) and race-
clean (TSAN). The one non-finding: doctests don't LINK under ASAN + build-std (a
toolchain quirk), so the target runs `--tests` (skips doctests).

**Fuzz grammar, i64-path edges.** Coverage showed two still-dark i64-arm paths:
`i64_guard_overflow` (an unmasked overflow that must deopt to a bignum, never
wrap) and `i64_throw_call` (a `throw` inside an i64 arm). Added an unmasked-
overflow pure recursion (`(* a base)` to a bignum) and a throw-in-recursion
(caught) to the pure-recursive generator section.

## 2026-07-16 — Reader/evaluator robustness fuzzer (adversarial input)

Added `stress/fuzz_reader.py` and wired it into `make stress`. The differential
fuzzer only makes VALID programs, so the reader's and evaluator's *error* paths on
malformed input were untested. This one feeds hostile input — random (possibly
non-UTF-8) bytes, unbalanced/blown-out delimiters, up-to-thousands-deep nesting,
truncated strings/escapes, hostile numerics (`1e999999`, `1/0`, `1.2.3`), giant
single tokens — and asserts the process always fails GRACEFULLY: never a crash
signal (SIGSEGV/SIGABRT), never a Rust panic (`.brood_crash_dump` / "panicked
at"), never a hang. A clean nonzero exit with a diagnostic is the correct outcome
and not a finding. Result: **3400 adversarial inputs + deep-nesting probes (10k /
50k / 200k parens) all failed gracefully** — the reader has a depth guard (no
stack-overflow SIGSEGV) and no unwrap/index panics on garbage. Runs in a dedicated
workdir so a concurrent differential sweep can't cross-pollute the crash-dump check.

## 2026-07-17 — Chasing mod.rs coverage: two optimization passes now fuzzed

llvm-cov showed the differential fuzzer reached only 70% of `mod.rs` (the VM
compiler + exec_chunk + dispatch). Triaged the 70 dark functions: most are
debug/test-only (`set_forced_engine`, `jit_verify_staged` — unreachable by
fuzzing by design), monomorphization instances, or slow rooted fallbacks hit by
volume. The valuable gaps were two semantics-preserving COMPILER OPTIMIZATION
passes the grammar never triggered (prime miscompile territory):
- **linmap** (linear map-accumulator → private mutable Table): fires only for a
  map threaded through self-recursion, updated via `map-int-add`/`map-dissoc` and
  read via `map-get`/`map-count` (the serializable whitelist — `map-assoc` is
  excluded). The grammar used `assoc`/`get`, so it never fired.
- **EA scalar replacement**: a single-binder `(let (v [..]) ..)` read only by
  constant `(nth v K)`, lifted element-wise into slots (vector never allocated).
  The grammar only did the immediate `(nth [..] k)` form.
Added generators for both; verified they fire and are correct (jit==tree). mod.rs
70.0→74.1%, jit_lower 74.6→77.5%, dark fns 70→55. The 4 soaks were restarted on
the new grammar so both passes now fuzz continuously. Remaining dark: the
deopt-resume machinery (`vm_resume_deopt` — already guarded by
jit_deopt_effects_test, and gated to non-self-loop/float-slot arms) and the
`hof_apply_native` JIT fast-path (narrow `apply`-with-closure trigger) — deep
internals with contrived triggers, left for a targeted pass if ever needed.

## 2026-07-17 — The targeted pass happened anyway: HOF driver + deopt/effect shapes

The two "left for a targeted pass if ever needed" dark spots from the entry above
got their pass the same morning (`5e54d01`):
- **`%range-reduce` HOF driver**: `reduce` with a NON-prim closure over a range
  routes through the native HOF driver + its JIT fast-frame
  (`hof_apply_native`/`hof_apply_step`) — a prim reducer (`fold` with `+`) skips
  it entirely, which is why the grammar never lit it. Confirmed firing (60k calls
  in a probe run) and correct.
- **Deopt/effect-ordering shape**: a `table-incr` effect before a non-tail call
  whose vector result is destructured — forces a deopt across a pending effect,
  fuzzing the checkpoint machinery that keeps effects exactly-once (the
  deopt-rerun bug's neighbourhood).
Both agree jit-vs-tree across 50 seeds; the soaks fuzz them continuously now.

## 2026-07-17 — Checker: a file's own defn now supersedes a builtin's signature

The bintree benchmark surfaced a checker false positive: a file defining its own
`check` (shadowing the `(check form)` builtin) still had its call sites typed by
the *builtin's* signature — "+: argument 2 expects number, got list" on every
run, plus the builtin's 1-ary arity leaking into arity checks. That violates the
ADR-123 contract (a def always wins; the checker never warns on a use valid for
the image's next state — the file is checked pre-load, so any existing heap
binding for a file-defined name is by definition the OLD value).

Fix: every heap-derived read in the checker is now gated on
`!ctx.is_file_global(s)` — the call-site sig + arity resolution and zero-arg
lint (walk.rs), the call-result type via `sig_of`/`declared_heap_overload`, the
numeric/seq by-name refinements, and the value-reference `global_value_ty`
reads (guards.rs, gradual_of). A file-redefined name is typed only from what
the file itself declares (`(sig …)`) or nothing — dynamic, never stale.
Regression: `file_defn_shadowing_a_builtin_wins_over_its_signature` (no stale
sig, no stale arity, and the *un*-shadowed builtin still warns). Also restored
the zero-warnings invariant across tests/: `rand-val` (message_roundtrip_test)
opts out via `check-allow :non-tail-recursion` (depth-bounded by design) and
jit_let_slot_reuse_test's deliberate dead binding is `_`-prefixed.

## 2026-07-17 — string->codepoints: the missing text-access primitive

The benchmark 7/7 chase quantified a shared bottleneck across every text row:
building the codepoint vector the parsers index. `(apply vector (map char->int
(string->list s)))` pays a 1-char string allocation per char, a closure call per
char (through `map`), and three passes — measured at **~40 % of the whole regex
benchmark** (134 of ~331 ms of match work), and the same shape sat under
`std/json`'s parse entry, `std/encoding`'s hex/base64 decodes (which also hashed
*1-char-string* map keys per char), and the prelude's `string-codepoints`.

Added the `string->codepoints` primitive: one O(n) `chars()` pass to a vector of
int codepoints. It clears the "genuinely needs Rust" bar the same way
`string-split`/`string-span`/`%str-index-of` did — char indexing into UTF-8 is
O(index), so pure Brood can't express the O(n) scan — and it's pure mechanism:
the regex/json/base64 parsers stay Brood. Rewired: `std/regex` (`regex--codes`
wrapper deleted, call sites use the primitive), `std/json` (`json-parse` entry),
`std/encoding` (hex + base64 alphabets, and both decode val-maps now key by
**int** codepoint instead of 1-char strings). The prelude `string-codepoints`
defn is deleted — the primitive replaces it under the arrow-convention name —
and its inverse is renamed `string-from-codepoints` → `codepoints->string`
(greenfield rename; callers updated; curated checker sigs follow).

Tests: strings_test's codepoints block extended (astral plane, cross-process
send/receive round-trip); regex/encoding/json suites green; nest check zero
warnings; full suite green.

## 2026-07-17 — spawn regression root-caused: the shared-arm compile flood

The 2026-07-17 benchmark rerun drifted `spawn` 48 → 68 ms; bisect landed it on
286e91f (dense Table + tier-on-resume + the sync-compile escape hatch). Phase
instrumentation put the whole loss in the **fan (spawn) loop** (23 → 45 ms,
bimodal), and `BROOD_COMPILE_TRACE` showed why: **`fib` compiled ~68 times** —
every short-lived process queues its OWN `CompiledArm` copy of the same shared
closure, the shared-cache publish only happened at the compiling process's
*next native run*, and the dequeue-side resolved-skip can't see across copies.
The flood is old, mostly-harmless background waste; what 286e91f changed is
that fan's own arm now hits the sync-compile escape hatch (QUEUED at a 2048th
back-edge) and **blocks on the GLOBAL_JIT module lock the flood was holding** —
compile latency moved onto the spawning process's critical path.

Three fixes, all keeping the queue free of runtime references:
- `jit_tier`: a QUEUED arm now still consults the shared cache — a peer's
  published code installs over QUEUED instead of interpreting until this copy's
  own compile lands (the dequeue resolved-skip then drops the stale queue entry).
- The background compiler keeps its **own** publish map — `(runtime_tag,
  share_key) → (code, epoch)`, a plain thread-local HashMap — so the Nth queued
  copy installs the first copy's code instead of lowering again. `runtime_tag`
  is a new plain-u64 id on `RuntimeCode`: the first cut passed the runtime
  `Arc` through the queue and **broke the single-process RUNTIME compactor**
  (its gate is `Arc::get_mut`; two runtime_collector tests caught it — a Weak
  would break it identically). Epoch-validated against the copy's enqueue-time
  `compile_epoch`; the runner's live-epoch guard re-checks on entry regardless.
- `jit_compile_now`: shared-lookup before taking the module lock — any valid
  published pointer ends the spin without compiling.

fib now compiles ONCE under a 10k-process storm; fan 45 → 22 ms; the plain
bench 107 → 87 ms (release-fast A/B) — at/under the pre-regression baseline.
Suite 784/784; runtime_collector 20/20; 500 fresh differential-fuzz seeds
agree across engines.

Also closed: the bintree drift (91 → 115 ms in the rerun) attributed to
9c81190's effect-safe checkpoints. Controlled A/B puts today's gap within
noise (min 145 vs 142 ms), and `BROOD_NO_DEOPT_RESUME=1` bounds the journaling
cost at ~5 ms on this shape. A sound skip needs transitive callee purity (a
non-tail call's completed callee may have effects) — not worth re-entering the
exactly-once soundness neighbourhood for ~4%; accepted cost, documented here.

## 2026-07-17 — Inlined-upgrade queue gets the same flood dedupe

The deferred (inlined-upgrade) compile queue has the identical per-process-copy
shape the primary queue's flood came from, so it got the identical fix
preemptively: a second thread-local publish map (`published_inline`) keyed
`(runtime_tag, share_key)`, consulted before lowering and written after. Kept
as a separate map from the small-arm one — a small-arm pointer must never
install into `inline_code` (different frame sizing, `inline_nslots`) and vice
versa. Also deflaked `jit_tier_compiles_a_hot_arm_then_runs_native` under
plain `cargo test` (one process, every test sharing the single background
compiler — the 400×2ms poll starved; nextest never sees it) and re-confirmed
the remote-spawn `after 5000` timeout is a parallel-load flake (fails ~1 in 5
full runs, passes alone and in clean runs). Suite 784/784; 300 fuzz seeds
agree.

## 2026-07-17 — regex leaves 7/7: cache split, vector hot-object, and a deopt storm

Three rounds took the regex row 279 → ~92 ms compute — past Clojure (103 ms),
off last place for the first time. The row was 67 % PER-CALL overhead (7.45 µs
each; the DFA loop itself only 0.245 µs/char), so the levers were:

1. **Cache split.** `regex--compile`'s memo hit was a `table-get` of the whole
   compiled object — and a Table read deep-clones the value out, including the
   `:states` NFA (a vector of per-state maps) on EVERY `matches?` call. The
   cache now holds a small hot object; the state vector lives in its own table
   (`regex--states-cache`) fetched only on a memo miss / exit-first-sight /
   the n = 0 edge. −1.3 µs/call.
2. **Vector hot-object.** The hot object became a 6-slot vector — a Table
   clone-out of a small vector is a flat copy (a CHAMP map rebuilds nodes) and
   the entry glue reads `nth`, not keyword lookups. compile-hit 1.2 → 0.47 µs.
3. **The deopt storm** (the real find — a general JIT gap). `BROOD_DEOPT_TRACE`
   (now printing the checkpoint's resume ip) showed the matcher loop compiled
   but deopted ~every 256 back-edges, running the whole 2M-char workload
   interpreted. Minimised to: a self-tail loop whose LAST self-call argument is
   an `if` expression strands the earlier args across the branch's block
   boundary, where the cross-block operand carry materialises a lazy `Op::Slot`
   as an int-guarded payload — an opaque handle (the pattern string, the two
   memo tables) fails the guard every iteration. The regex loops now compute
   the branch in a let binder (empty operand stack at the boundary; every
   self-call arg a simple slot read) — per-char cost 0.245 → 0.056 µs (4.4×).
   **Engine follow-up recorded:** per-leader stack-shape analysis (meet of
   `Slot(k)` across predecessors → keep the slot lazy through the boundary)
   would make the natural nested-if style equally fast; until then any
   self-tail loop threading an opaque value through an arg-position branch
   hits this cliff. The `[deopt] resume_ip` trace addition is the diagnosis
   tool for the next one.

Checksums bit-identical across VM / no-JIT / tree-walker; regex tests 14/14;
suite 784/784; nest check clean.

## 2026-07-18 — bintree: the checkpoint tax measured honestly, and a purity exemption

Chasing the bintree drift (91 → 110 ms across mid-July) with wall-clock A/Bs
kept returning noise; `perf stat -e instructions` (the load-independent metric,
per docs/benchmarking.md) settled it: HEAD ran **+6.5 % instructions** over the
ed502ba baseline on bintree, and `BROOD_NO_DEOPT_RESUME=1` recovered ~5 % —
the 9c81190 checkpoint journaling after every completed non-tail call (two
self-calls per node × 819k nodes), previously judged "flat" from wall alone.

Fix: a **pure-self-recursion exemption** in `jit_ckpt_depth`. An arm whose
every `Call` (tail or not) targets itself, with no `table-put` inline prim and
no `try`/`catch`, is effect-free by induction — a deopt's from-ip-0 re-run
re-executes only completed self-calls of this same pure arm (a mid-run
redefinition bumps the epoch and invalidates the arm first). Such arms skip
checkpointing: no journal slots reserved, no per-call journal stores. Anything
that can reach an effect — a non-self call (natives live there), a computed
callee, `table-put`, a catch frame — keeps the exactly-once machinery.
bintree instructions 1.788G → 1.711G (+1.9 % vs baseline, from +6.5 %).

Also: **minor-collect nursery capacity seeding** (`Slabs::with_capacity_like`).
A collection installed `Slabs::default()` — zero capacity — so every cycle
re-paid the Vec-doubling ladder up to the threshold (each doubling memmoves
everything so far). The fresh nursery now reserves the outgoing nursery's
lengths (the steady-state high-water mark; a spike's excess capacity is
released next cycle). Neutral on bintree's wall (collections are rare there —
the memmove in its profile is per-call `push_n` frame staging, intrinsic call
machinery), but removes the ladder for any workload that collects often.

What bintree's profile says is LEFT: ~17 % `jit_run_fast_link` + ~11 %
`push_n`/frame staging (the boxed non-tail call path — the "true call
inlining" FRONTIER lever), ~10 % allocation FFI. Those are the deferred
big-ticket JIT items, not regressions. Validated: effects test, GC_STRESS
checksum, suite 784/784, 300 fuzz seeds across engines.

## 2026-07-18 — Docs brought back to 100%: the full staleness sweep

A five-cluster audit of every living doc against the code (concurrency/dist,
VM/JIT/perf, language/core, types/LSP, editor/misc), then fixes across ~40
files. The recurring root causes, so the next sweep knows where drift
accumulates:

1. **The 2026-06-08 state-capture cutover (ADR-100) had never been folded back**
   into the docs that predate it. `scheduler.md`, `concurrency.md`,
   `memory-model.md`, `architecture.md`, `components.md`, `testing.md`, and
   `ROADMAP.md` all still described corosensei coroutines, fresh-only stealing,
   pinned processes, or "work-stealing deferred" as current. All now state the
   shipped reality (general stealing + live migration, dirty-block carve-out)
   and mark the coroutine era as history.
2. **ROADMAP lagged the July perf sprint**: the 7/7 priority set (nbody, regex,
   sieve, persistent-map) is cleared — rows updated with the dated fixes; the
   "runtime housekeeping" items (tracing GC, work-stealing) and the **package
   manager** (`:git` deps + verbs shipped 2026-05-30 — packages.md was right,
   the roadmap wasn't) flipped to ✅; the stability backlog now credits the
   cargo-fuzz targets + stress kit and the `run_one` catch_unwind.
3. **Gap B0 (2026-07-10) reverted-then-shipped literal precision** was still
   described as "tried, reverted, deferred" in five type docs; all now carry
   the B0 resolution. types.md's intro adopts the revised contract-#5 phrasing
   (never gates the live image; `nest check` is the batch-only hard reject).
4. **Path drift**: `eval/compile.rs` → `eval/compile/{mod,ir,jit_lower}.rs` and
   `builtins.rs` → `builtins/` fixed in every living nav pointer (~15 docs);
   `let*` removed from the three docs still listing it; stale "16-byte
   `Value`" → 24 bytes where presented as current.
5. **Docs that undersold shipped features**: mcp.md (17 tools incl. write/edit;
   process-scoped stdout capture), gui-font-gaps.md (all gaps resolved —
   ADR-079 `:scale`, per-window `gui-font!`), linear-map-accumulator.md and
   type-map-kv.md (marked shipped), primitives.md (bytes/table/codepoints rows
   added, false "(100)" count dropped).

Historical records (devlog, decisions, archive/, dated audits/postmortems)
were left as-is per the dated-narration rule; transients.md's overruled
"Phase 2 shipped" block got an explicit REVERTED banner instead of deletion.

## 2026-07-18 — Stack traces in error values (BEAM/.NET gap #1 closed)

First item off the 2026-07-18 runtime-survey list ("Robustness gaps vs BEAM /
.NET"). Every kernel error now carries the call stack at the raise, surfaced as
`:trace` on the caught error map (innermost-first `{:fn [:file :line :col]}`
entries, capped at 32) and printed as `at fn (file:line:col)` lines for
uncaught errors (CLI report + REPL).

Design: an entry pairs a frame's fn name with the **call site that entered
it** — buildable from purely local frame state in both engines, and tail calls
collapse into their caller's frame identically (the BEAM behaviour, and an
honest picture of the real O(1)-stack frame structure).

- **VM**: `attach_vm_trace` at `vm_run_bc`'s error-return sites walks `cur_arm`
  + the live `BcFrame`s — a caller's saved `ip` is the return address, so
  `code[ip-1]` is the `Inst::Call` carrying the call-site pos. `CompiledArm`
  gained unconditional `fn_name` (any named closure — distinct from the
  jit-only `dbg_name`, whose symbol keys semantic tables) and `src_file`
  (from `form_pos` at compile time). Nested drivers (native callbacks) each
  append their frames as the error crosses them. Also fires for the
  memory-limit / deadline / `MAX_BC_FRAMES` raises — a recursion-depth error
  now shows the cycle.
- **Tree-walker**: `eval` split into a thin wrapper + `eval_tail_loop` carrying
  an `entered` tracker; one entry per eval frame that entered a closure body
  (tail entries rename the frame, the first entry keeps the call site —
  matching VM frame reuse exactly). `apply_closure` runs its last body form on
  the loop with the tracker **seeded**, so native-boundary callbacks merge
  with their tail chain instead of double-counting.
- **Parity**: information-free synthetic frames (no name/file/pos — the
  top-level program thunk, anonymous boundary thunks) are suppressed in
  `push_trace`; `error_format_parity` (4 tests) green, engines byte-identical
  on the corpus. JIT'd arms trace via their deopt re-raise (verified
  jit/no-jit/TW identical on hot-loop + deep-recursion errors).
- **Bonus fix**: the ADR-135 program-exit seam (`ProgramExit`) carried
  `Err(String)` — a flattened `located()` line — so `brood FILE` errors had
  **lost the caret/hint since the top-level-as-green-process cutover**. It now
  publishes the structured `LispError` (payload stripped at the process
  boundary), restoring the full report and carrying the new trace.

Tests: `tests/try_catch_test.blsp` §11 (6 cases: innermost order, tail
collapse, call-site fields, 32-cap, native-boundary naming, helper-chain
attribution) — green on VM/TW/GC_STRESS+VERIFY. Follow-ups tracked in
ROADMAP/tasks: structured error (incl. `:trace`) in process death reasons;
per-process resource limits (survey gap #2) is next.

## 2026-07-18 — Per-process heap limits: `(process-flag :max-heap n)` (survey gap #2, lever 1)

Second item off the runtime-survey list. Before this the only memory cap was
the global ADR-043 pair, whose *hard* tier aborts the **whole OS process** —
BEAM kills just the offender. Now a process can cap itself:

- **Mechanism** (kernel): `Heap::proc_mem_limit` (per-process, default
  unlimited) + a `note_proc_limit` check at the end of **both** collection
  paths (legacy flip + generational), where the slabs hold exactly the
  survivors — so the figure is *live* data and transient garbage a collection
  reclaims never trips it. O(1) (slab lens × sizes), no-op unless set. Over
  the limit arms a **sticky flag** probed at the four eval/VM safepoints
  (tree-walker loop top, `vm_run_bc` loop top with trace attach, both
  `exec_chunk` SelfCall safepoints), which raise a catchable `E0045`
  ("process heap limit exceeded", hint included) in that process only.
  Sticky matters: a JIT-resident register loop can't allocate (out of
  subset), so any heap-growing path necessarily revisits a safepoint; a
  parked/preempted resume also passes the probe.
- **API** (Erlang `process_flag/2` shape): `(process-flag :max-heap n)` sets
  (positive int), `nil` clears (also cancels a pending trip — a `catch` that
  clears the limit genuinely rescues the process), absent reads; returns the
  previous value. Unknown flags error and name the known set — future flags
  (`:max-mailbox`?) slot in.
- **Policy** (Brood): no spawn option — cap a worker by setting the flag
  first thing in the spawned fn.

Lever (2), mailbox bounds, stays deferred per ADR-011 (drop-vs-error-vs-park
has real design surface, and remote delivery can't error the sender) until a
concrete consumer picks the policy.

Tests: `tests/process_limit_test.blsp` — flag protocol, catchable E0045,
rescue-after-clear, capped-process-dies-alone (parent + uncapped sibling
unaffected) — green on VM / tree-walker / no-JIT / GC_STRESS+VERIFY.

## 2026-07-18 — Death reasons carry the structured error (trace follow-up)

Completes the stack-trace item across the process boundary: an uncaught error
now retires its process with reason `[:error {:kind :message [:code :file
:line :col :hint :trace]}]` instead of `[:error "<flattened string>"]` — the
same map a `catch` binds, so a monitor's `[:down …]`, a trapping link's
`[:EXIT …]`, and a supervisor all get BEAM's `{Reason, Stacktrace}`.
`message::error_reason` builds it directly as a heap-independent `Message`
(the dying heap is about to drop), so it deep-copies into any receiver and
crosses the dist wire (Map/List are wire-encodable). Shape-agnostic consumers
(`[:error _]` matches, supervisor `:transient` policy) were untouched; the one
string-exact assertion (chaos_test) updated, plus a new test proving `:trace`
frame names survive into the receiver's heap. Suite 784/784 (2782 in-language),
both engines.

## 2026-07-18 — Link propagation carries the originating reason (survey housekeeping)

The last exit-signal gap from the runtime survey: link propagation hard-killed
a non-trapping peer with a literal `:kill`, so the peer's monitors — and every
further cascade — reported `:kill` instead of why the tree fell. Root cause: a
kill's **hardness** (die at the next reduction tick vs the next `receive`) was
keyed off the *reason value* (`is_kill_reason`), conflating the two.

Fix: hardness is now a property of the request — `MailboxState.kill_hard`,
`request_kill(reason, hard)` — with `(exit pid :kill)` hard-with-`:kill` as
before, and `links::deliver_exit_to` requesting `exit_propagate` = hard with
the **originating reason** (BEAM semantics; with the same-day death-reason
work, the whole chain now reports `[:error {… :trace}]` end to end). The
sticky-latch guarantee ("a racing soft exit can't downgrade a kill") keys on
the hard flag; the loop-top probe is `pending_hard_kill()`. Remote links ride
the same `deliver_exit_to`, so cross-node propagation is fixed too.

Also un-staled `docs/language.md`, which still claimed "bidirectional links
are not implemented yet" (ADR-067 shipped long ago) — it now has a Links
section documenting trap/propagate + the reason-carrying cascade.

Tests: two new link_test cases (propagated reason; chain cascade), the sticky
unit test extended with the hard-with-soft-reason case; exit/supervisor/serve
suites unchanged-green.

## 2026-07-18 — Dirty-CPU accounting: long natives charge reductions + named stalls

Survey gap #6, the cheap half. A CPU-bound native can't be preempted mid-call;
now it at least pays for the time it held the worker: in a green process
(`in_capture_run` — one TLS bool, the root thread pays nothing), `call_native`
times the call and `scheduler::charge_native` debits the reduction budget at
~2 reductions/µs (saturating: ≥~1 ms of native work drains the 2000-reduction
quantum), so the next safepoint yields promptly instead of the process keeping
its quantum as if the call were one reduction. And the `BROOD_STALL_MS` tracer
— which could name minor-gc/compaction/quantum but never the actual builtin —
now logs `[stall] native <name> took Nms` at the call site.

**Revised same day by a fuller A/B** (the first measurement used a JIT-fused
bench that never exercised `call_native` — a bad probe): always-on per-call
timing cost **8–22% on the message-heavy rows** (pingpong +22%, ring +11%,
json +8%; confirmed by neutralize-and-rerun), while the fairness win was
marginal — reduction preemption already bounds post-native hogging to ~one
quantum (~1 ms of Brood work), and the un-preemptible time *inside* a long
native is only fixed by the M4 offload pool. So the per-call timing+charging
is now **gated behind `BROOD_STALL_MS`** (the diagnostic mode where the named
stall trace lives); the default path pays one TLS bool + a cached-env load,
and the full-row A/B is flat. Unit test:
`charge_native_drains_reductions_proportionally`. The offload pool stays the
M4 item.

Also shipped today, smaller: `mapv`/`filterv` (hatch ergonomics — prelude
one-liners, vector-returning `map`/`filter`; tests + language.md), and the
`let`-vector-destructure-of-a-list hatch item was verified to already raise a
clean match-error (the "or erroring" arm of the ask — resolved).

## 2026-07-18 — Dist self-healing: net/reconnect + opt-in :send-errors (survey gap #5)

The last mid-size survey gap. Two halves, mechanism/policy split:

- **Kernel seam:** `dist::route` now reports whether a route existed (local, or
  remote with a live link) vs the message being dropped for an
  unknown/disconnected node; `process::send` turns that into a catchable
  **E0060 noconnection** error when the sending process opted in via
  `(process-flag :send-errors true)` (the third `process-flag`). Default stays
  Erlang-silent, and process *liveness* stays silent either way — the flag is
  about the link.
- **Brood policy:** `std/net/reconnect` (bundled, `(require 'net/reconnect)`) —
  one named, idempotent watcher process per connect spec: connect, arm
  `monitor-node`, and on `[:nodedown]` retry `(connect spec)` on exponential
  backoff (`:min-ms` 500 → `:max-ms` 30000), then re-arm and notify
  subscribers `[:nodeup name]`. A stale/duplicate `[:nodedown]` while still
  linked is ignored (re-armed monitors can double-fire). ~90 lines of Brood
  over existing primitives — no new kernel surface beyond the send signal.

Test: `reconnect_watcher_heals_a_fallen_link` (cli::distribution) — B falls, A
sees `[:nodedown]`, an opted-in send raises E0060, B restarts on the same
port, the watcher heals the link, A gets `[:nodeup]` and a registered-name
send round-trips. Also un-staled language.md's dist section ("monitors/
node-down deferred" — they shipped long ago) and documented the net-split
send semantics.

## 2026-07-18 — Review pass over the day's work: ensure-link consolidation + fixes

Self-review of the whole day's diff (the code-review sweep). Findings + fixes:

1. **Duplication caught: the prelude already had `ensure-link`** (a fixed
   200ms-backoff reconnect supervisor) — the roadmap's "nothing reconnects"
   was stale, and today's `net/reconnect` overlapped it. Consolidated per the
   one-coherent-design rule: **`ensure-link` removed from the prelude**
   (helpers too; `sleep` kept), `std/net/reconnect` is the sole reconnector,
   the `ensure_link_reconnects_across_a_node_restart` test ported to
   `net/reconnect/watch` (green), distribution.md/decisions.md updated with
   supersession notes.
2. **Watcher mailbox hygiene:** the reconnect watcher is a registered-name
   process but its `receive`s had no catch-all — arbitrary messages would
   have accumulated forever. Both states now drop unknown messages (the same
   discipline the old ensure-link loop had).
3. **Verified non-issues:** `monitor-node` is per-pid **deduped** in the
   kernel, so the watcher's re-arm after each reconnect cannot accumulate
   monitors; remote link exits funnel through the same `deliver_exit_to` as
   local ones, so the hardness/reason split behaves identically across the
   wire; MCP error JSON derives from `to_value_map` by construction, so
   `:trace` flows through it; the test framework flattens caught maps to
   kind+message, so failure output stays clean.

## 2026-07-18 — Benchmark regression sweep over the day's work

Full A/B (release `--bin brood`, HEAD-worktree baseline binary vs the working
tree, best-of-N wall time across the 18 `brood-benchmarks` rows): after the
`charge_native` gating fix above, **all rows are flat within noise** —
pingpong/ring/json recovered from +22/+11/+8% to ±2-4%, loop/sieve/bintree/
nqueens/errors-deep/persistent-map/spawn/base64/reduce/startup unchanged, and
the sub-100 ms rows' ±10% single-run wobbles vanish at 7 reps. The named
stall tracer (`BROOD_STALL_MS=5` → `[stall] native %range-reduce took 69ms`)
still fires when armed. Method note for next time: a "native-call-heavy"
probe must be verified to actually route through `call_native` — the first
probe (`reduce +`/`bit-and` loops) was JIT-fused to ~1.5 ns/iter and measured
nothing.

**Archive caveat (same day):** the fresh divan archive
(`docs/benchmarks/2026-07-18T15-57-30Z.md`) was taken on a thermally
saturated laptop (package 99 °C against a 100 °C limit, `powersave`
governor, hours of builds prior) — unrelated micro-rows read ~2× the
cold-machine 2026-07-11 archive (fib-VM, maps), while the encoding rows
show the July sprint's real −35…−58%. Cross-archive comparison is
meaningless under that skew; the interleaved same-conditions A/B above is
the regression instrument that counts (flat). Lesson recorded: archive runs
belong on a cold, `performance`-governor machine.

## 2026-07-18 — Feature-parity push: Erlang timers land; two "missing" OTP items were already shipped

Working the BEAM/.NET feature list top-down (perf deferred to a second pass):

- **Timers** — the genuinely missing piece: `send-after` / `send-interval` /
  `cancel-timer` in the prelude, pure Brood (a timer is a green process parked
  on the scheduler's timer wheel — `sleep`'s mechanism — so pending timers
  cost no worker thread; the handle is the timer's pid). `send-interval`
  monitors its target and exits when it dies (no orphan tickers);
  `cancel-timer` is an idempotent hard exit. `tests/timer_test.blsp` (7 cases,
  loose timing bounds for loaded CI).
- **Stale roadmap discoveries:** the other two "OTP near-term" items already
  existed — `remote-spawn-sync` (returns the remote child's pid) and the
  `[:$stop]` graceful-teardown convention (supervisor `:shutdown`
  `:brutal-kill`/`:infinity`/ms policies + `defprocess` `terminate`). Roadmap
  ticked accordingly.

Remaining feature-parity gaps after this: the observability timing tier
(pause durations / event stream / sampling profiler — next), the
parked-waiter leak, and the deliberately consumer-gated set (gen_statem,
Registry/pg, Application, inbound TLS, mailbox bounds — the last arguably
not a parity gap at all: BEAM doesn't bound mailboxes either).

## 2026-07-18 — Observability timing tier, slice 1: GC pauses, sched counters, a sampling profiler

Survey gap #4's two named holes ("no pause times, no Brood-level CPU
profile") closed:

- **GC pause durations** — `Heap::collect` is now timed (two `Instant` reads
  per *collection* — noise against the collection itself; only recorded when
  `gc_runs` actually moved) into per-process total/max/last, surfaced as
  `(gc-stats)`'s `:pause-total-us`/`:pause-max-us`/`:pause-last-us`.
- **Scheduler counters** — new `PREEMPTED`/`EXITED` atomics (quantum
  exhaustions in `handle_capture_outcome`, exits in `deregister`) join the
  existing spawn/steal/migrate counts behind one `(sched-stats)` snapshot map.
- **Sampling CPU profiler** — `crates/lisp/src/profile.rs` + a frame-boundary
  probe in `vm_run_bc`: a ticker thread bumps an epoch at the requested rate
  (`profile-start [hz]`, default 99); each driver compares a loop-local
  last-seen epoch at its safepoint and, on change, records its reified
  named-frame stack (cur + pending `BcFrame`s — the data ADR-100 already
  reifies) into a global histogram; `(profile-stop)` returns
  `{:stack (…) :count n}` entries, most-sampled first. No signals, no
  unwinder; off = one relaxed bool load per frame boundary. JIT-resident
  loops attribute at their reduction-budget preempt (~once a quantum); the
  legacy tree-walker isn't sampled (documented). Start/stop cycles retire the
  ticker via a generation counter — no thread leaks.

Tests: `tests/observability_test.blsp` (structural, engine-aware — the
profiler content assertions gate on the VM engine). This closes the kernel
*sources* half; the ⬜ remainder is the ADR-106 event-stream unification
(gc/sched/deopt/dist events consumable by `nest observe`/mcp), `defevent`
schemas, aggregators, and the remote tier.

## 2026-07-19 — Observability slice 2: the kernel event stream (`system-monitor` → telemetry)

The ⬜ half of the timing tier: runtime events are now *consumable*, not just
countable. Design (ADR-137): BEAM `system_monitor/2` shape — **push, not
poll** — the kernel delivers each selected event as an ordinary mailbox
message `[:system kind subject-pid detail]` to ONE subscriber process, via
the same `process::deliver` seam monitors/dist already use. No ring buffer,
no new wait primitive.

- **Kernel** (`process/sysmon.rs`): `(system-monitor pid opts)` arms; events
  `:gc` (emitted after `Heap::collect`, detail `{:pause-us :collections
  :live}`, filtered by `:gc-min-pause-us` — BEAM's `long_gc`), `:spawn`
  (detail = parent), `:exit` (detail = the structured reason monitors see —
  rides the existing `deregister` reason), `:deopt` (the VM driver's
  `Some(1)` outcome branch; detail = arm fn name). Guards: events about the
  subscriber itself are never emitted (else its own GC would feed itself
  forever), and `deregister` disarms when the subscriber dies (a dead
  monitor must not keep charging every event site). Off = one relaxed
  `AtomicBool` load per site.
- **Policy** (`std/telemetry.blsp`): `watch-runtime` spawns a watcher that
  arms the monitor on itself and re-emits each kernel event as a
  `[:runtime kind]` telemetry event — so operators observe the runtime
  through the exact ADR-106 attach/handler seam their app events use.
  `stop-watch-runtime` (or killing the watcher) disarms.
- **Tests**: `tests/sysmon_test.blsp` (8 cases: arm/clear round-trip,
  opts selection, spawn/exit/gc event shapes, the `long_gc` threshold,
  self-exclusion + death-disarm, and the end-to-end telemetry flow). Green
  on the VM and under `BROOD_GC_STRESS` — the stress run caught a real test
  bug (an isolated unit's tests share one worker mailbox, so a previous
  test's event backlog must be flushed). Under `BROOD_VM=0` the runner's
  :isolated+`receive` combination misbehaves — **pre-existing** (maps_test
  hangs identically there at the pre-change binary), noted in the test file.

Still open on this axis: node up/down through the same stream, `defevent`
schemas, aggregators, `nest observe`/mcp consuming it, the remote tier.

## 2026-07-19 — Cold-start measured: it's macro expansion (27 of 31 ms), not eval

Scoping the startup-image-snapshot roadmap item (ReadyToRun analogue) began
with instrumentation instead of a design: a permanent `BROOD_BOOT_TRACE=1`
flag now prints the shared-prelude build's phase breakdown (works in
release). Result, three stable runs:

    builtins 0.3 ms · read 2.3 ms · macro-EXPANSION 27 ms · eval 0.9 ms · freeze 0.7 ms

The assumed shape ("serialize the frozen SharedCode bundle") was aimed at the
wrong cost: evaluation + freeze together are ~1.6 ms. The entire boot cost is
`eval::macros::compile` — hundreds of interpreted macro invocations (every
`defn` runs the `defn` macro body on the tree-walker), spread evenly across
the prelude (the 14 slowest forms >300µs sum to just ~6.5 ms; no single
pathological form — the exponential match-lowering class was already fixed
2026-07-16).

Consequences for the roadmap item (updated there): the preferred lever is now
**making macro expansion itself fast** (e.g. running macro bodies on the VM) —
a general capability that speeds every `require` and reload, exactly the
ADR-006 "build the language up" shape; the fallback is an expanded-prelude
disk cache (print post-expansion forms, key by ADR-129 build-id) which reaches
~6 ms with no binary heap format. Full SharedCode serialization is
unnecessary.

**Follow-up (same day) — second-level split; the "make expansion fast" lever is
NOT cheap.** A temporary `BROOD_BOOT_SPLIT` instrumentation (reverted after
measuring) split the 28.9 ms `macroexpand_all`: **744 expander invocations,
25.1 ms** (~34 µs/call avg); the recursive walk itself 3.8 ms; `resolve` 7 µs;
static-quasiquote expansion 1.1 ms. Key correction to the entry above: macro
bodies ALREADY run through the active engine (`apply_engine`, the ADR-119
work) — the cost is genuine Brood list-churn inside the macro bodies
(`defn`, `match` pattern lowering), i.e. the same allocation-heavy-VM frontier
as `pipeline`/`nqueens`, not a dispatch gap. The startup item's practical
lever is therefore the expanded-prelude disk cache (~6 ms, build-id-keyed);
the ROADMAP entry now says so.

## 2026-07-19 — Fix: tree-walked frames are native for capture purposes (TreeWalkGuard)

The morning's "pre-existing TW runner hang" diagnosed and fixed. Root cause:
`CAPTURE_TOP_LEVEL` is maintained by `vm_run_bc` (set on the top-level body
driver, cleared for nested VM runs) — but **entering the tree-walker never
cleared it**. So under `BROOD_VM=0`, TW code reached from a capture-mode
driver (a `%isolate`/`%try`/HOF callback via `apply_engine`'s TW branch, or a
VM tw-defer) ran with the driver's stale `true`: a parking `receive` inside it
took the CAPTURE path instead of the mandated §7.4 block. The suspend signal
unwound the un-reifiable native/TW frames, the capture resumed at the
bytecode call instruction — and **re-ran the whole native thunk**, repeating
its side effects (the §8.1 footgun verbatim). Visible as the test runner
re-running `:isolated` bodies (fresh spawns each round, children killed by
`%isolate`'s reaper) until the 120 s hard kill; invisible (but real) wherever
the re-run happened to be idempotent.

Fix: `process::TreeWalkGuard` — an RAII clear-and-restore of
`CAPTURE_TOP_LEVEL`, entered where frames genuinely become tree-walked:
`eval::eval` (form evaluation — tw-defer, the BROOD_VM=0 program branch) and
`eval::apply_closure` (closure bodies applied from natives/HOFs). Nested
entries are no-op re-clears. A TW-nested `receive` now blocks its worker
exactly like any native-nested receive.

**Placement matters — the first cut caused a real regression.** Guarding all
of `eval::apply` (including its Native branch) cleared the flag around the
VM's *dispatch-fallback shim* too, and `(%receive …)` reached through that
shim is still bytecode-reachable: every VM receive became a worker-blocking
one. Caught immediately by two Rust suite tests —
`deep_receive_continuations_resume_correctly_across_workers` ("no live
migration observed") and `runtime_drain::parked_process_clean_…` — capture
and migration were effectively disabled. The guard moved to `apply_closure`
(the actual TW body evaluation, which `eval` does not cover — it runs
`eval_at`/`eval_tail_loop` directly), and the Native branch stays unguarded.

Minimal repro (fixed): green process → `%isolate` thunk → `println` +
`receive` under `BROOD_VM=0` printed twice, now once. Validation: the
previously-hanging `--test` runs under BROOD_VM=0 now pass whole —
maps 67/67, sysmon 8/8, gen 18/18, concurrency 33/33, capture 8/8,
dynamic 16/16 — the full VM suite 786/786 (including the two
migration/drain canaries), and a 150-seed differential-fuzzer batch
(tree-walk leg included).

## 2026-07-19 — Boot cache shipped: ~38 ms → ~6.5 ms cold start (ADR-138)

The morning's measurement chain (31 ms boot → 27 ms is expansion → 744
expander calls of genuine Brood work, no dispatch fix) ends in the predicted
lever, implemented: the **expanded-prelude boot cache**. `boot_from_source`
now prints each post-`compile` prelude form (expanded + resolved +
static-quasiquote) and writes them to
`~/.cache/brood/prelude-expanded-<hash(build-id)>.blsp` after a successful
boot; `boot_from_cache` reads them back and skips `eval::macros::compile`
entirely. Measured on this machine (release): **source boot 38.6 ms (incl.
cache write) → cache hit 6.5 ms**; `BROOD_NO_BOOT_CACHE=1` opts out (32.8 ms,
the pre-cache baseline).

Safety rails, per ADR-138: `build-id` as the staleness key (header +
filename — per-binary files since the id embeds each executable's mtime; ~7-day
age-prune of stale siblings), a per-form print→read→print fixpoint gate before
anything is written (one unprintable form poisons the write; the boot just
stays on source), delete-on-any-failure fallback to the source boot, the
caching boot's gensym counter floored at cache boot so runtime gensyms can't
collide with cached expansions, and a positioned raw-prelude read on the cache
path so `note_definition`/LSP `M-.` are identical on both paths. Writes go
temp-file + rename for nextest's many-processes-one-binary boot storm.

Validation: `make test` **786/786** green with the cache active — and under
nextest every test runs in its own process off one binary, so after the first
shard writes the cache virtually the whole suite *boots through it*, making
the run itself a broad cache-boot correctness check (the Brood suite's 96 s
in-language pass included).

## 2026-07-19 — Leaf-callee inlining behind BROOD_JIT_LEAF_INLINE (~30% on helper-loop shape)

The deferred Phase-2 lever from `docs/jit-optimizing-tier.md` implemented as the
self-inliner's sibling: a non-tail call to a statically-known, small, calls-free
top-level `defn` is spliced into the caller (`LetBind` binds the args into the
callee's shifted slot range; `shift_slots` relocates the body above the caller's
frame), removing that call's whole protocol. Derivation happens ONCE at
arm-compile time — the only moment with `&Heap` to resolve callee symbols — and
is stored on the arm (`CompiledArm::leaf`), riding the existing two-stage
deferred-upgrade channel (`inline_name`/`inline_nslots`/`inline_code`, mutually
exclusive with self-inlining).

Hot-reload correctness is the interesting bit: the stored derivation is
epoch-stamped, and `jit_lower_inlined_arm` refuses to lower it at any other
epoch. Any `def` between derivation and lowering (or after install, via the
per-entry `compile_epoch` guard) invalidates; the arm falls back to its small
native permanently until its closure recompiles. Tested: a post-warm `def` of a
spliced callee takes effect (late binding exact).

Three bugs found and fixed building it, each a general lesson:
1. **Probe reentrancy walks the whole call graph.** Resolving a callee compiles
   it, whose own probe resolves *its* callees … and never terminates on mutual
   recursion (instant boot stack-overflow). Fixed with a thread-local
   `LEAF_RESOLVING` guard: a nested compile skips probing (depth 1 by
   construction — a qualifying callee is calls-free anyway).
2. **The inlined engine must not read the small layout's deopt checkpoint.**
   `ckpt_slot` sits at `scope.max + spill` — INSIDE the spliced slot range,
   where a callee's Int param faked a packed journal → garbage resume ip →
   capacity-overflow panic. `jit_ckpt_read` now refuses when
   `inline_installed` (a latent hazard for the self-inliner too — its block 1
   starts at `m` — now closed for both). Derivation therefore requires ZERO
   residual non-tail calls, keeping the from-ip-0 deopt re-run effect-free by
   construction.
3. **`inline_nslots` must be floored at the small frame size.** The small
   `nslots` includes spill + checkpoint reserves, so a lean spliced layout came
   out SMALLER — and the per-engine sizing hook's "grow to `inline_nslots`" on a
   post-swap entry became an underflowing shrink (`extend_roots_to_nil`
   capacity overflow). Floored at construction for both inliners.

Measured (release, da=off): the target shape `(+ acc (sq (add1 i)))` ×5M:
**1.65 → 1.20 s (~30%)**. Benchmark-suite rows (fib/bintree/nqueens/loop/
collatz/spawn/pipeline) flat — as diagnosed, those are recursive/HOF/alloc-bound,
not scalar-helper-bound; reaching them needs closure-arm support (the v1 gate
requires a `defn` name for the swap's fast-link invalidation) and Phase 3/4.
Gates green WITH the flag: JIT≡VM differential 28/28 under
GC_STRESS+GC_VERIFY, VM≡TW differential, three new `tests/jit.rs` cases
(exactness across the small→leaf swap, hot-reload redef, residual-call gate),
full `make test`. Opt-in until measured on expansion/`require`-heavy loads;
flip to `BROOD_NO_LEAF_INLINE` opt-out then.

## 2026-07-19 — Mailbox receive: one lock per matched message (was three)

The `receive` fast path (message already queued / just delivered, first
candidate matches — pingpong's every iteration) took **three** mailbox-mutex
acquisitions per matched message: peek + `from_message` **under the lock**,
a second lock to remove the matched message, and a third clearing a
`recv_deadline` that a nil-timeout receive never persisted. Now one:

1. **Optimistic pop for the first candidate of a scan** (`scan_mailbox`):
   take the message out under the one lock, run `from_message` and the
   matcher with the mutex *released* — a match is done with no second
   acquisition, and `send` never contends with the deep copy into the
   receiver's heap. Sound because only the owner removes from its own
   mailbox (send only appends), so the position is stable across the
   unlocked window. A non-match re-inserts at the same position (arrival
   order preserved — and on a matcher *error* too, so an erroring matcher
   can't lose the candidate) and reverts the rest of the scan to
   peek-in-place: a long selective-receive backlog isn't popped/re-inserted
   per candidate, keeping every scan's lock count ≤ the old scheme's.
2. **`recv_deadline` clears only when this receive persisted one**
   (`deadline.is_some()`): a nil-timeout capture-mode receive skips the
   lock — the previous receive's exit already left the slot `None`.

Measured (release, interleaved A/B vs HEAD baseline built in a worktree,
min-of-5): **pingpong (N=2M) 4.80 → 4.60–4.70 s (~2–4%, new ≤ base in all
five pairs)**; ring flat (its per-hop cost sits in copy + capture/restore,
the roadmap's remaining lever). Small because uncontended lock ops are
~20 ns against a ~2.4 µs RT — this closes the "double mailbox mutex-lock
per matched msg" increment; the copy-trim for small scalar messages stays
the next rung. Gates: full `make test`, CI-equivalent clippy, targeted
proc/gen/supervisor/message-roundtrip smoke on the release binary.

## 2026-07-19 — `type-of` as `PrimOp1::TypeOf`; a type-mixed-join JIT miscompile found and fixed

**Where the ping-pong time actually goes.** gdb SIGINT-sampling (perf and
valgrind are both unavailable on this box; `gdb` as parent + external
`kill -INT` loop works under `ptrace_scope=1`) attributed the ~2.35 µs RT:
`to_message`/`from_message` is ~2% — the roadmap's "trim the copy" lever is
NOT worth chasing for pingpong. The weight is the receive **matcher execution
path** (`hof_apply_step → vm_apply → vm_run_bc` per candidate, plus the
matched clause allocating its body thunk closure per message) and the
scheduler. En route, the samples showed `type-of` dispatching through the
tree-walking `eval::apply` fallback inside matcher arms.

**`type-of` is now a compiled prim** (the `Sqrt` discipline): total over every
operand — a tag read + the per-tag cached keyword in the VM, and in the JIT a
256-entry discriminant-byte → keyword-id table load (`jit_layout::
type_of_kw_table`, built from one dummy-handle exemplar per `Value` variant so
the collapsing rules `BigInt`→`:int`, `Range`/`SeqView`→`:pair` hold by
construction; an exhaustive match forces a new variant to update it).
Compile-time-known operands (int/float/bool consts) fold to constant keywords;
no shape deopts. Epoch-guarded like every prim, so a user `(def type-of …)`
still wins (tested). This is what every type predicate bottoms out in
(`vector?`/`int?`/… are one-line Brood wrappers, hit per candidate by
`match`/`receive` dispatch and per element by the seq predicates) — and it
makes those wrappers' bodies call-free, i.e. leaf-inlinable (2026-07-19 leaf
entry). Measured: a 3-way type-dispatch loop **1.41–1.65 → 1.10 s (~25–30%)**;
pingpong ~1–2% (only one side's matcher tests a shape); all benchmark rows
flat-or-better.

**The new test's failure was a real, pre-existing JIT miscompile.** The
hand-written expectation "diverged" — but TW = VM = 180000 while the JIT gave
180344 *and the untouched HEAD binary gave 180364* (deterministic each).
Minimal repro (no `type-of` involved): `(defn code (x) (if (%eq x 7) 1 (if
(%eq x true) 5 0)))` fed `(if (%eq (rem i 2) 0) 7 (< 1 2))` — a **join whose
edges disagree on scalar typing** (unboxed int vs i8 comparison). Each edge
jumping to a join *overwrote* `bool_param[t]`, so the LAST-lowered edge's
typing won for every edge. `BROOD_JIT_VERIFY_FN=code` then showed the smoking
gun in one line: `arg[0] = bool raw=[0x1,0x7,0x0]` — the int edge's raw 7
staged as `Value::Bool(7)`. (Mistyped the other way, the bool edge strips to a
raw truthy `Int 1`.) The branch-condition side of this ambiguity was already
known and deopted (the `nest format` non-idempotency fix); the param-boxing
side was unsound. **Fix:** the first edge to reach a join fixes its typing
(`record_block_flags`); a later edge whose flags disagree routes to `deopt`
(args dropped) — the VM runs that iteration on the real tagged value. All
three edge sites (`Jump`, both `JumpIfFalse` successors, leader fall-through)
now agree-or-deopt. Rows flat; `pred_loop`/repros bit-identical across
TW/VM/JIT.

Lesson repeated from 2026-07-19 (leaf entry): verify hand-computed test
expectations against the interpreter FIRST — this time the "wrong arithmetic"
was a genuine engine divergence, and the discipline of checking TW/VM/JIT
separately is what surfaced it.

## 2026-07-19 — The no-JIT build compiles again (and CI now keeps it honest)

`cargo build --no-default-features` — the documented build-level counterpart of
`BROOD_NO_JIT` — had rotted: 18 errors, all in `eval/compile/mod.rs` (the
background-compiler machinery — `JitCompiler`/`JIT_COMPILER`/the `crate::jit`
refs inside it — plus the `jit_ckpt_depth` call) landed without `#[cfg(feature
= "jit")]` gates, and nothing built the configuration. Fixed with the existing
stub discipline (`jit_ckpt_depth` gets a `None` stub beside `jit_spill_reserve`'s
zero stub; the compiler struct/static gated whole), plus three warning cleanups
(`tick_capture_n` re-export + defn gated — only the JIT callback calls it;
`RuntimeCode.runtime_tag` allowed-dead in no-jit, kept unconditional so the
struct layout doesn't fork). Verified: no-jit workspace check clean, the no-jit
binary runs the smoke programs + proc/gen/message test files, bit-identical to
the tree-walker on the type-of/mixed-join repros. CI's build-test job gained a
`cargo check --workspace --no-default-features` step — a seam like this can't
rot silently again for the cost of one cached check.

## 2026-07-19 — Leaf-callee inlining flipped to default ON (`BROOD_NO_LEAF_INLINE` opts out)

The morning's "opt-in until measured on expansion/`require`-heavy loads" gate is
met. Measured flag-on vs flag-off on the same binary: **cold start** (boot
cache; 100×-hello batch 0.89 vs 0.88 s), **`require`-heavy load**
(editor/buffer + sexp + json + regex, 20 ms both), **`nest check` over std/**
(~0.5 s both), **the in-language suite wall** (13.6 s both), and **every
benchmark row** (fib/nbody/json/wordcount/sieve/loop/bintree/nqueens/spawn/
pipeline/collatz) — all flat. The wins hold and now COMPOUND with the same
day's `PrimOp1::TypeOf`: scalar-helper loops ~30% (1.5–1.6 → 1.10–1.15 s), and
type-predicate dispatch a further ~8% on top of the prim (predicates'
bodies are call-free now, so `vector?`/`int?`-class wrappers splice into hot
matcher/classifier arms — the exact shape `match`/`receive` dispatch runs).

The flip inverts the lever to the self-inliner's convention:
`leaf_inline_enabled()` defaults true, `BROOD_NO_LEAF_INLINE=1` is the A/B /
bisect opt-out (CLAUDE.md env table row added; ROADMAP item ✅). The
tests/jit.rs leaf cases now exercise the shipping default (the
`enable_leaf_inline` env helper is gone). Verified the lever both ways on the
target shape (1.15 s default, 1.56 s opted out). Gates on the new default:
full `make test`, tier_transition + metamorphic differential fuzz rounds
(armed build, fresh seed base), clippy/fmt.

## 2026-07-19 — Iolists at the write boundary (ADR-139)

The roadmap's "highest-leverage" stability item: `tcp-send`, `proc-send`,
`spit`, `spit-append`, `spit-bytes`, `append-bytes`, and `bytes-concat` now
accept arbitrarily nested string/bytes/byte-int trees, flattened exactly once
at the write by one shared iterative walker (`flatten_iolist` — immutability
means no cycles, so no visited set; explicit worklist, so 40k-deep nesting is
heap-bounded, tested). Additive: every one of these previously REJECTED lists,
so no behavior changed for existing callers — `spit-bytes`/`append-bytes`'s
old bytes/vector/list-of-ints surface is a strict subset of iolists.
Binary-mode sockets keep the Latin-1 byte-string rule per string LEAF.
`str`/`join` deliberately stay display-rendering (recorded in the ADR).
Checker signatures widened to the shallow iolist union (the `Ty` lattice can't
express recursion; the runtime flattener owns the leaves). Docs: language.md
§Iolists, PRIMITIVE_DOCS rewritten for all six, ADR-139. Tests: an iolists
describe-block in `tests/bytes_test.blsp` (nesting, empties, improper tail,
rejects, spit/append round-trips, the 40k-deep case), green on VM and TW.
Next rungs from the same roadmap cluster: port the HTTP/WS parsers off the
carrier-string bridge, and the growable read buffer (input-side twin).

## 2026-07-19 — Iolists follow-up: the deep-nesting test found a real kernel limit

CI's `brood_suite_passes` died SIGABRT — `fatal runtime error: stack overflow`
— on the new "40k-deep nesting flattens iteratively" test. The flattener is
NOT the culprit (explicit worklist, by construction): the depth stresses the
kernel's **recursive heap walkers** — GC tracing / `promote` recurse per pair
natively, so a deep-but-legal immutable value can blow a worker's stack if a
collection lands while it is live (nondeterministic: the same test passed
locally and on one CI-shaped rerun — it depends on when GC fires). Filed as a
roadmap housekeeping item (make the tracers iterative, like the flattener);
the test caps at 2k depth meanwhile — still deep enough to prove the
flattener's worklist, shallow enough for the recursive tracers.

## 2026-07-19 — std/net/http on iolists; a real Content-Length bug fixed

Dogfooding ADR-139 immediately paid: porting `render-response`/`render-head`
to return wire **iolists** surfaced that `Content-Length` was computed with
`string-length` — **codepoints, not bytes** — so any non-ASCII body
("héllo ☃" = 7 codepoints, 10 UTF-8 bytes) shipped an understated length and
spec-honoring clients truncated it. Now the body is materialised ONCE
(`bytes-concat` — so `:body` may be a string, a `bytes` value, or any iolist
tree; binary bodies ride the plain-response path), `Content-Length` is that
bytes value's count, and the length and the sent bytes can never disagree.
Bonus fix en route: a non-Latin-1 string body over the server's binary-mode
socket used to trip the per-codepoint byte-string check inside the one big
wire string; as a flattened bytes leaf it now goes out verbatim as UTF-8.
The client's request `Content-Length` got the same byte-count fix (its
docstring had honestly documented the approximation; no longer needed).
`http--render-headers` deliberately stays a string (small, and `sse.blsp` +
the client compose it into string requests — `tls-request`'s Rust signature
takes a string). Tests: render cases materialise via
`bytes-concat`/`utf8-bytes->string`; new cases pin the byte-count
Content-Length and a binary `bytes` body end-to-end. 792/792.

## 2026-07-20 — Deep-value stack safety: segmented growth in the recursive heap walkers

The 2026-07-19 filing closed: `promote_in`, the GC `flush_value`, `equal`, and
`hash_value_into` each recurse per **car**-nesting level (their cdr spines were
already iterative), so a deep-but-legal immutable value — `(def deep <60k-deep
nested list>)` died in `promote_in` the moment it was defined; the same value
under churn died in the GC copy; `(= deep deep2)` died in `equal`; a deep map
key died in hashing. Four deterministic repros, one fix: each recursion entry
now checks remaining native stack and grows in heap-backed segments
(`stacker::maybe_grow`, 64 KB red zone / 1 MB chunks — rustc's own approach;
new dep justified in Cargo.toml per the runtime-crate bar). The alternative —
rewriting four bottom-up builders (promote/flush/equal/hash × pair/vector/map/
closure/env) as explicit two-phase stack machines — was rejected as far more
complexity and risk for the same guarantee. Scalar fast paths keep the guard
off the hot compare/hash paths (`equal` returns before the guard for
immediate/scalar pairs; `hash_value_into` skips it for scalar keys); the
`verify_local_graph` debug walker was already worklist-based, `to_message` is
depth-capped (256) and errors cleanly, and the printer survives 60k deep
as-is. New `tests/deep_values_test.blsp` pins promote/GC/equal/hash at
20k–60k depth (each test in its own green process, so worker stacks are the
ones proven); the iolist deep-nesting test is restored to 40k.

## 2026-07-22 — Bit syntax: typed integer segments in the bytes pattern (ADR-140)

Tier 1, item 1 of the new runtime-feature parity program (documented at the
top of ROADMAP.md today). The `(bytes seg…)` match pattern — which already
existed byte-granular (and undocumented; now in `docs/language.md` §Bytes
patterns + the two grammar tables) — gains **typed integer segments**:
`(x :u16)`, `(x :i32-le)`, `(_ :u32)`-skip; u/i × 8/16/32/64 × be/le,
big-endian default. Pure Brood end to end: `match-bytes-typed-seg` lowers
onto new prelude reads `bytes-uint`/`bytes-uint-le`/`bytes-int`/`bytes-int-le`
(1–8 bytes at an offset, over `byte-at`) and encoders
`int->bytes`/`int->bytes-le` (truncating, so `(int->bytes -1 2)` =
`#b"\xff\xff"`) — zero new Rust, no kernel surface. A pleasant discovery en
route: an unsigned 8-byte read past `i64` **auto-widens to a big integer**
(ints promote transparently now — the ROADMAP "no bignums" out-of-scope line
was stale and is fixed), so `:u64` has exact Erlang semantics with no caveat.
Deliberately deferred (ADR-140): sub-byte bit widths, float segments, UTF-8
segments. Tests: two new describe blocks in `tests/bytes_test.blsp` (typed
segments incl. TLV-driving-sized-segment, non-linear typed binders, a
cross-process parse-and-send case; helper round-trips both endians), green on
VM, tree-walker, and no-JIT; `nest check` stays zero-warnings. Next rung:
port the `std/net` HTTP/WS parsers onto bytes + these patterns (kill the
carrier-string bridge).

## 2026-07-22 — The parser port: std/net bytes-native, carrier strings deleted (ADR-141)

Tier 1 item 1's second half. The kernel rule change that unlocked it: **binary
mode now governs only the inbound decode** — `tcp-send`/`proc-send` string
leaves are ALWAYS UTF-8 (`flatten_iolist` loses `latin1_strings`; the
codepoint-as-raw-byte Latin-1 send rule and its >U+00FF error are gone —
raw bytes ride as `bytes` values). That made binary-for-life sockets free:
the **http server** no longer flips back to text after reading a request (the
flip-window race class — the original U+FFFD bug's shape — is structurally
gone; SSE frames and user stream-fns send strings unchanged). The **http
client** now reads in binary mode: response `:body` is byte-faithful `bytes`
(new `body-text` decodes text bodies), request bodies may be
string/`bytes`/iolist (https stays string — `tls-request` is a string seam in
both directions, documented; fix rides the server-mode TLS work), and
`parse-response` is a bytes parser. `tcp-drain`/`-timeout` return `bytes`
(chunks joined once — the reversed chunk list is an iolist). **SSE read loops
deliberately stay text-mode**: `text/event-stream` is UTF-8 text and the
kernel's decode (longest-valid-prefix + multibyte carry) is the right framing
— recorded in the module header. Stale Latin-1 docstrings scrubbed from
net.rs/proc.rs/reader.rs/PRIMITIVE_DOCS. Tests: new client-side
binary-response e2e (an octet-stream body of 0xFF/0x80/0x00 through http-get,
byte-exact — impossible before), non-ASCII body round-trip, bytes POST via
http-post, ADR-141 send-rule pins in tcp/proc tests (a `π` proc-send in
binary mode now delivers `0xCF 0x80` instead of erroring); the old
Latin-1-carrier client trick in the binary-request e2e replaced with a
`bytes` iolist leaf. tcp/http/sse/proc/bytes files: 104/104; full suite
green; `nest check` zero warnings.

## 2026-07-22 — Identity: general-purpose language; README/CLAUDE.md/ROADMAP reframed

"This is just brood now. It developed a bit more than the initial intent."
The docs no longer frame Brood as a *small* language built solely as an
editor substrate: it is a general-purpose language and runtime with a
deliberately small **core** (the design principle stays), and the editor is
origin story. README rewritten accordingly (also dropped the last stray
reference to the old separate editor-app project).

## 2026-07-22 — Tier 1 items 2+3: no read-buffer transient (ADR-142); the socket reactor (ADR-143)

**ADR-142** closes the "growable read buffer" item by design: a buffer value
is a transient (ADR-026 forbids it, permanently), and the chunk-list +
join-once idiom is already O(n) — what was quadratic was the head reader's
per-chunk *rescan*, now incremental (`bytes-index-of :from`, backing up
marker−1 bytes across chunk boundaries) with a 64 KiB head cap; both pinned
by tests (a dripped head whose `\r\n\r\n` straddles chunks; a 70 KiB
terminator-less head is dropped).

**ADR-143** rebuilds `crate::net` on one mio reactor thread — plaintext
streams, TLS client + server, and listeners all as reactor state machines,
replacing thread-per-socket (and the TLS actor's 10 ms poll, and the accept
loop's 2 ms nap). Same mailbox contract end to end; the deliberate semantic
changes: `tcp-send` queues (drain-before-close kills the send-then-close
truncation footgun; 16 MiB cap bounds a stuck reader; write errors surface
as `[:tcp-closed]`), peer half-close leaves the write side usable (Erlang),
and TLS became a first-class stream — `tcp-set-binary` honored everywhere,
`tls-request` takes iolists + an optional `ca-pem` trust anchor, and
`http-get`/`http-post` gained `:ca` and are byte-faithful over https (the
ADR-141 seam is closed). `serve-loop` handed a `tls-listen` socket serves
https with zero changes — pinned by the new e2e. Tests: `tests/tls_test.blsp`
is the first in-tree end-to-end TLS coverage (handshake round trip, binary
mode, iolist requests, clean `[:tcp-error]` on an untrusted cert), plus two
https-through-the-std-server cases in http_test; 56 net tests + suite
792/792, VM/TW + GC_STRESS green, `nest check` zero warnings. `SubscriberHandle`
and the per-source thread machinery for sockets are gone (`sink_pair` cells
replace retargeting); `proc.rs` keeps `spawn_io_source` for subprocess pipes.

## 2026-07-22 — Tier 1 item 4: the dirty-native offload pool (ADR-144) — Tier 1 complete

BEAM dirty-scheduler parity via the ADR-059 seam, no scheduler surgery:
`%offload` copies an allow-listed blocking native's args out as messages,
runs it on a small OS pool (≈nproc/4, min 2) against a private scratch
`Heap`, and delivers `[:offload token result]` / `[:offload-error token err]`
back; the prelude `offload` wrapper parks in a selective receive on the token
and rethrows errors. Allowed: long/blocking data-in/data-out natives only
(git, kdf, digest/hmac, file IO, TLS keygen) — anything heap-sharing is
refused at the call. `package.blsp`'s `%git-clone`/`%git-resolve-ref` ride it
(a `nest fetch` no longer pins a worker); the ADR-071 WASM gate is open.
Tests `tests/offload_test.blsp` (7: round trips incl. file IO, error
rethrow, refusals, selectivity — a decoy message survives the wait — and an
8-process concurrent fan-in), green on VM/TW/no-JIT/GC_STRESS; suite
792/792; `nest check` zero warnings.

**Found en route — a real freeze/expansion wart:** a prelude `defn` whose
body uses the `receive` macro AND is defined *after* the macro expands at
boot, and that boot-time expansion leaves a closure with a captured local
frame in the boot slab — `freeze_as_shared_code`'s global-env assert then
kills boot. Every pre-existing receive-using prelude fn (sleep, send-after,
send-interval) sits *before* the macro, so their bodies expand lazily at
first call and the constraint was invisible until `offload` broke it. The
fix here is placement (offload sits with its siblings, comment explains
why); the underlying wart — boot-expansion of a receive matcher creating a
freeze-hostile intermediate — is filed in the stability backlog to be either
fixed (GC the boot slab before freeze, or make the assert reachability-
based) or given a boot-time diagnostic that names the offending form.

## 2026-07-22 — The freeze wart fixed: reachability-aware dangling-env check

The morning's filing closed the same day. Root cause confirmed: the builder
heap never collects (dense, stable indices are what make the local→prelude
re-tag a pure bit-flip), so the closure slab at freeze also holds boot
*garbage* — and the dangling-env assert swept all of it. A boot-time
`receive`-matcher expansion legitimately creates a closure capturing a local
frame while the expander runs; dead by freeze, it still tripped the assert.
Fix: a **mark pass** (iterative worklist from the global bindings over
pairs/vectors/CHAMP nodes/closure arms/env chains) classifies each closure;
**reachable** ones keep the hard assert — a live captured frame really would
dangle once the env slab is wiped — and **unreachable** ones get `env`
scrubbed to `None`, which nothing can observe. Measured: exactly **1**
scrubbed closure with `offload` boot-expanded (`BROOD_BOOT_TRACE=1` prints
the count), confirming the diagnosis. The prelude `offload` moved to its
natural home *after* the `receive` macro — deliberately, so every boot now
regression-tests the fix; the before-the-macro placement convention is dead.
Gates: offload/bytes/tcp/http/tls files green, suite 792/792, GC_STRESS
clean, `nest check` zero warnings. (Separate, noted not fixed: boot garbage
is *frozen into* the shared prelude region — a size cost, not a correctness
one; a compacting freeze would need handle rewriting and is not worth it
until the prelude's frozen size matters.)

## 2026-07-22 — Validation pass over the day's kernel changes

The reactor (ADR-143), offload pool (ADR-144), and freeze fix all cleared
the full arsenal: **`make stress` 32/32** (3-oracle differential program
fuzzer 25 seeds × 4 configs, chaos preemption, GC-stress passes,
checker-soundness corpus, cross-language table digest, 1500 adversarial
reader inputs), and a **reactor scale soak** the suite doesn't attempt —
one `serve-loop`, 400 then **1000 concurrent `http-get` clients** in one
runtime (both ends green processes), **zero failures** at both scales
(~0.9 s / ~3.7 s wall in a debug build; the old model would have needed
~2N OS threads — the reactor runs it on one poll thread + the worker
pool).

## 2026-07-22 — WASM component interop, slice 1 (ADR-145): sandboxed native extensions

The ADR-071 design note becomes running code — the biggest remaining
capability item, unblocked by yesterday's offload pool. The kernel embeds
`wasmtime` (Component Model, fuel metering) behind a default-on `wasm`
feature (the `jit` precedent): `%wasm-load` instantiates a component from
bytes or WAT text, `%wasm-call` calls an export with args marshalled by the
export's OWN WIT parameter types (ints range-checked per width, floats,
bools, chars, strings, lists, tuples, options; results lift records→maps,
variants→tagged vectors, enums→keywords, WIT `result` errors raise),
`%wasm-exports`/`%wasm-close` round it out. A runaway guest hits the fuel
cap and raises a catchable error — the sandbox holds by construction (linear
memory only; no WASI wired: pure compute, deny-everything). An instance is
an opaque token (mutable state → handle behind primitives, never a sendable
Value); calls serialize per instance; `%wasm-call` is offload-allowed so a
long guest call parks the process, not a worker. Policy is `std/wasm.blsp`:
`wasm-load`/`wasm-instantiate`/`wasm-call`/`wasm-call-blocking`/`wasm-exports`/
`wasm-close` and **`use-native`** — every export `def`d as an ordinary Brood
fn, no hand-written stubs. En route the checker gained the
**`(check-allow :unbound …)`** category (runtime-defined globals the source
checker can't see — exactly what `use-native` produces). Tests are
hand-written **WAT components** (wasmtime parses WAT — no toolchain needed):
scalars, a memory+realloc guest proving strings (incl. UTF-8) and
`list<s32>` cross the canonical ABI byte-faithfully, the fuel-meter trap,
error paths, `use-native`, 8-process concurrent calls, an offloaded call —
11/11. Deferred slices per the ADR: package-manager `:native` integration,
WASI grants, guest resources, epoch preemption, blob zero-copy.

## 2026-07-23 — Finish-the-partials batch 1: embedded teardown, checker hardening, fuzz targets, and a repo-wide build bug

The "finish everything not 100% done" sweep, first batch — three long-open
items closed and one surprise:

**Embedded-host teardown (the parked-waiter leak).** `Interp::drop` now runs
`shutdown_runtime_parked`: every permanently-parked `receive` waiter of the
dropped runtime is taken from its mailbox slot (under the state lock — racing
sends are safe) and routed through the normal `deregister` death path
(monitors fire, links propagate, names/sockets clean). Runtime-scoped by
`Arc::ptr_eq` on the heap's `RuntimeCode`, so co-hosted runtimes are
untouched. Pinned by `crates/lisp/tests/interp_teardown.rs` (reap on drop;
another runtime's parked process survives and stays wakeable).

**Checker host-panic hardening.** `check_file` runs its whole analysis under
`catch_unwind` with compile-ns/known-names/imports/GC-roots restored on both
paths — a checker panic degrades to one "checker internal error" diagnostic
instead of killing brood-lsp / `nest check`. And deep-but-legal CODE no
longer blows the native stack: the recursive walkers (`check_into`,
`collect_def_names`, `check_recursion`, `check_macro_hygiene`,
`collect_syms_into`, and the expander's `macroexpand_all_depth` +
`resolve_walk`) grow the stack in heap-backed segments — the code-side
sibling of the 2026-07-20 deep-value fix, driven by gdb backtraces of a
30k-deep-form test that aborted the host walker by walker until it passed.

**The three missing fuzz targets** (JSON via a persistent `Interp`; the dist
wire decoder — the unauthenticated surface; the bundle footer/archive) ship
with two workflow fixes that made them *usable*: the fuzz dep is lean
(`default-features = false`, no wasmtime/cranelift in the sancov tree) with
`system-alloc` (ASAN must own allocation — mimalloc under interception ran 4
execs/min), and `make fuzz T=<target>` sets `ASAN_OPTIONS=symbolize=0`
because the system llvm-symbolizer stalls ~90 s at EVERY exit against the
65 MB instrumented binary (diagnosed via /proc wchan: main thread parked on
`anon_pipe_read`). First smoke: wire 7.9M execs/min, bundle 54M, json 134k —
zero findings across ~62 M executions.

**The surprise: every build of every profile was recompiling `brood`.** The
build script declared `rerun-if-changed=.git/HEAD` as a *relative* path —
which resolves against `crates/lisp/`, where `.git` doesn't exist, and cargo
re-runs a build script every time a watched path is missing. Nearly invisible
under incremental dev builds; ~2 minutes per `cargo fuzz` invocation is what
exposed it. Now absolute + emitted only when the paths exist: a repeat fuzz
invocation went 2 min → 0.29 s, and every workspace rebuild stops paying the
tax.

## 2026-07-23 — Clojure/Scheme teaching hints (reader-level)

**LLM-native reader hints.** The reader used to mis-parse three common
Clojure/Scheme reader macros into confusing downstream errors: `#{1 2 3}` →
"map literal has an odd number of forms", `#(+ 1 %)` → "unbound symbol: #",
`#'foo` → same. Now `read_hash` catches `#{` / `#(` / `#'` and raises a clean
parse error carrying a `:hint` that names the Brood idiom (the set library,
`(fn …)`, plain `'foo`) — while `#b"…"` and bare `#foo` symbols still read.
And Scheme/Clojure's nested `let`/`letrec` bindings `((a 1) (b 2))` (odd
count) now raise a hint to flatten to `(a 1 b 2)`, detected by the
all-elements-are-`(name value)`-pairs shape (a genuinely-flat odd `(let (a)
…)` gets no false hint). `tests/reader_hints_test.blsp` (8 cases, hints are
catchable in-language via `read-string`); the "Coming from Clojure" table in
language.md gains rows.

## 2026-07-23 — Validation pass, round 2: a remotely-triggerable server crash + 3 more

A second wave of adversarial reviews (bit syntax, the bytes-native parser port,
the offload pool + module privacy) plus probing. Bit syntax came back
**clean** (signed/endian/i64-min/round-trips/widths all correct). Four real
fixes:

- **HIGH — a truncated HTTP request head crashed the server worker and leaked
  its fd** (ADR-141 port regression). A client sending a partial head with no
  `\r\n\r\n` then closing made `http--read-until` return the partial accumulator
  *without* the terminator, so `http--read-raw` did `(subbytes head 0 -1)` →
  threw → the per-connection process died before its `tcp-close` → leaked fd.
  Remotely triggerable, a mild fd-exhaustion DoS. The old string path used
  `http--split-first` (no-separator-safe); the bytes rewrite missed the
  `marker < 0` guard that `parse-response` already had. Fixed in
  `http--read-raw` (missing terminator → nil → clean close) and the sibling
  `parse-request` (headerless input parses instead of throwing — its docstring
  promised it worked). `http_test.blsp` gains a truncated-head e2e + a
  headerless `parse-request` case.
- **Privacy bypass via `(quote ~(private))` inside a quasiquote.** The
  level-aware walk (added this session) treated `quote` as terminal at *any*
  level, but Brood splices an `~unquote` nested inside a quoted subform of a
  quasiquote (`` `(quote ~(m/priv--x)) `` evaluates the unquote). Now `quote`
  short-circuits only at level 0; inside a quasiquote it keeps walking so the
  nested unquote is still checked. `private_test.blsp` pins it (10 cases,
  incl. nested/double-unquote levels and same-module macro templates).
- **A panicking offload-pool worker permanently drained the pool** — no
  `catch_unwind`, no respawn, so a native that panics (vs returns `Err`) killed
  its worker; with ~nproc/4 workers a couple of panics would hang every future
  `offload` (incl. `nest fetch`) forever. Now wrapped like the scheduler's
  green-process containment: a caught panic delivers `[:offload-error …]` and
  the worker survives. (No allowlisted native panics today — defensive.)
- **Privacy walk allocated per qualified symbol** — it built `format!("{m}/")`
  + interned + cloned to resolve aliases before testing `bare.contains("--")`,
  so every public `mod/name` ref paid it. Reordered to short-circuit non-`--`
  names first.

## 2026-07-23 — Adversarial validation pass over the day's work: 7 real fixes

Three parallel adversarial reviews (the net reactor, the wasm host, the
finish-the-partials Rust) plus hands-on probing turned up seven genuine
issues; all fixed, none left open above LOW.

**Sandbox / DoS (the important ones):**
- **`canonicalize` didn't resolve a `..` in the non-existent tail** — it broke
  out of the ancestor loop on the first `..` (`Path::file_name()` is None for
  `..`) and fell to a lexical-only fallback that *skipped symlink resolution*.
  So `canonicalize("/link/x/../../../etc")` returned the deceptive
  `/real/x/../../../etc` (which `starts_with("/real")` accepts) instead of the
  true `/etc`. A real bare-`starts_with` sandbox-escape hole in the primitive
  (the MCP caller was safe only because `mcp--safe-rel?` rejects `..` first).
  Rewritten to canonicalize the longest existing prefix, then resolve the
  symlink-free tail's `..`/`.` against it — the result is now `..`-free and
  safe. `mcp_sandbox.rs` gains the escape regression.
- **WASM had no memory limit** — fuel meters instructions, not space, so a
  component declaring a huge `memory` (or one `memory.grow`) could OOM the
  host for ~1 fuel unit. Added a per-store `ResourceLimiter` (256 MiB cap) and
  a 64 MiB load-input cap. `wasm_test.blsp` pins that a 327 MiB-memory
  component is denied at load.
- **TLS outbound had no `OUT_CAP`** — the plaintext path capped queued bytes at
  16 MiB but the TLS path fed rustls's writer unboundedly, so a stuck HTTPS
  reader grew `sendable_tls` without limit (the ADR's slow-reader bound was
  silently false for TLS). Added `pending_out` accounting that drops the
  connection past `OUT_CAP` while backed up.

**Correctness:**
- **`nest format --changed` silently skipped new files in a new directory** —
  plain `git status --porcelain` collapses a wholly-untracked dir to `?? dir/`;
  a `.blsp` filter then dropped it. Added `-uall`. Regression in
  `format_changed.rs`.
- **Host-panic hardening missed `collect_register_sig_forms`** — a deep
  `(do (do …))` chain could still SIGSEGV it (a stack overflow `catch_unwind`
  can't catch), defeating the pass. Wrapped in `stacker::maybe_grow`;
  `checker_survives_pathologically_deep_forms` gains a deep-`do` case.
- **`%git-changed-files` threw when git couldn't spawn** — a box without `git`
  errored `nest format --changed` instead of falling back to whole-project.
  Now maps a spawn error to `:not-a-repo`.
- **Poison-tolerant wasm locks + a nested-`option` rejection** — the wasm
  registry/instance mutexes now recover from a poisoned guard (a one-off panic
  can't turn every future call into a hard panic), and an ambiguous
  `option<option<T>>` marshal is rejected rather than silently collapsing `nil`.

**Documented, not code-changed (LOW / by-design):** a peer half-close leaves a
plaintext socket's fd until an explicit `tcp-close` (documented in
`std/net/tcp.blsp` — the serve-loop's per-connection process reclaims it on
exit); a WASM instance is a manual resource with no GC finalizer yet
(documented in `std/wasm.blsp`); `mcp-progress` only fires on the dispatcher
thread. Deferred as tracked follow-ups: a WASM instance finalizer (process/GC
reap), TLS half-close symmetry + lossy close_notify under backpressure.

## 2026-07-23 — MCP streaming/progress tier

A long `nest mcp` tool (a `check` over a big project) used to be one silent
wait. Now a `tools/call` carrying the MCP `_meta.progressToken` arms a
progress sink: the Brood handler calls `(mcp-progress progress total
message)`, which lands as a `notifications/progress` JSON-RPC message on the
same stdout stream the client reads — *during* the synchronous call. The
server is synchronous over stdio, so live streaming from inside a blocking
handler works via the **reentrant stdout lock** (writing from the handler is
safe even though `main_loop` holds the lock). `%mcp-progress` (kernel: a
thread-local sink armed by the dispatcher; a no-op returning false when no
token is in scope, so the same handler is safe anywhere) + a `mcp-progress`
wrapper; the core `check` tool (`check-project-structured`) reports per-file
(`checked foo.blsp`, 3/12). Tests: `progress_notification` shape (unit), a
`tools/call` with a token streams a notification, and without a token none
is sent (a test-only stdout redirect makes the reentrant-lock path
observable). Suite 803/803.

## 2026-07-23 — LLM-native MCP tools: explain-error + find-pattern

Two of the "errors that teach" tools shipped, as a new baked-in `explain`
module (`std/tool/explain.blsp`) — curated Brood data, policy-in-Brood:

- **`explain-error`** maps a stable error code (`E0044`), a caught error map
  (via its `:code`/`:kind`), or a kind keyword (`:type`) to
  `{:code :summary :causes :fix :example}` — the material was in the Rust
  error-code doc comments; this surfaces it to an agent as the actual *fix*
  (E0044 → "rewrite as a tail-recursive loop with an accumulator"), not just
  the message. Every one of the ~16 shipped E-codes has an entry.
- **`find-pattern`** keyword-searches an intent→idiom cookbook (loop / mutable
  state / build-a-string / set / map-update / error / spawn / receive /
  destructure / parse-binary / offload / private) — the "how do I X in Brood"
  an LLM reaches for, answered with the idiom + a runnable example + a doc
  pointer, so the reflex is the Brood way, not a Clojure/Scheme one.

Both are wired as `nest mcp` tools (the surface is now 20). Pure Brood over
string ops; `tests/explain_test.blsp` (9) pins the catalogues incl. a
completeness check (every entry has a summary + fix), `mcp_test` (5) pins the
tool wrappers + catalogue membership. Suite green; docs/mcp.md tool table
updated.

## 2026-07-23 — Finer type/arity finding spans (LSP/`nest check`)

A type-mismatch or
callback-arity finding anchored at the call head — so `(string-length (+ 1
2))` underlined `string-length`, not the actually-wrong `(+ 1 2)`. It now
anchors at the offending **argument** when the argument is a positioned
sub-form (the reader positions pairs, so a nested call gets a precise span),
falling back to the call form only for a bare literal/symbol (the pair-keyed
position table records no position for those, and the call head is the
closest anchor anyway). One `arg_pos` helper, no `Pos`-threading rewrite —
the argument `Value` already carries its position. `type_check_catalog.rs`
pins the column (`(+ 10 20)` at col 16, the lambda callback at col 6, a bare
literal falling back to col 1).

## 2026-07-23 — Symlink-escape-proof MCP write sandbox (`canonicalize`)

The `nest mcp` write/edit tools
gated paths purely lexically (reject absolute/`~`/`..`) — which the code
itself flagged as missing symlink resolution: a project-relative, `..`-free
path could still resolve *out* of the tree through a symlinked directory
inside it (`proj/link -> /etc`, then write `link/passwd`). Fixed with a new
`canonicalize` primitive (real absolute path — symlinks and `.`/`..`
resolved; works for a not-yet-existing target by resolving the longest
existing ancestor and appending the tail) and a second gate `mcp--under-root?`
that compares the canonicalized target against the canonicalized project root.
`crates/lisp/tests/mcp_sandbox.rs` pins the escape rejection and the
canonicalize resolution (unix-only; the private is reached from top-level
eval, the ADR-146 live-hacking hatch).

## 2026-07-23 — `nest format --changed`

Whole-tree `nest format` reformatted every
`.blsp`; `--changed` narrows to only the files git reports not-committed-clean
(modified/staged/untracked). New kernel mechanism `%git-changed-files dir`
(runs `git status --porcelain -z` from the repo top, returns absolute paths —
or the keyword `:not-a-repo`, distinct from a clean tree's empty list, since
an empty Brood list is nil); Brood policy in `std/format.blsp`
(`format-project-changed`) intersects with the project's own file set and
falls back to the whole project outside a git repo. `--check` deliberately
still scans everything (CI's clean-tree gate can't narrow). Integration test
`crates/nest/tests/format_changed.rs` (clean tree considers 0; one dirty file
is the only one formatted; non-git fallback).

## 2026-07-23 — "Private should be private": module privacy enforced (ADR-146)

The `--` convention becomes real semantics. From inside a module, a
hand-written qualified reference to another module's `--` name (plain or
aliased) is a **compile error at load**; `(:use mod :only […])` refuses
privates. Three doors stay open by design: **`(:use-internals mod)`** — the
`@testable import`-style explicit grant for tests/tooling (rides the import
table under the impossible key `/internals/<mod>`, the `%alias` trick);
**top-level/REPL code** (no namespace) — the live-hacking hatch;
and **a module's own macros** may expand to its privates anywhere, because
enforcement reads the *pre-expansion* source and skips `quote`/`quasiquote`
(the test framework's `describe`/`test` → `test/test--run` expansion made
that the only coherent rule — post-expansion enforcement flagged every test
file in the tree). Reflection still sees the flat table: a source-level
contract, not value-level sealing.

Enforcement immediately triaged the whole tree: **14 genuinely-shared
helpers got promoted to public API** (net/http `parse-url`/`request-headers`/
`render-headers` — sse's handshake stops reaching into http; lineedit's
embedding quartet `lineedit-init`/`-handle`/`-overrides`/`-remember` — the
observer and REPL are real embedders; project's model six
`project-find-root`/`-abs-paths`/`-collect-sources`/`-apply`/`-parse-dep`/
`-parse-deps` — the whole tool family consumes them; format's
`format-cst-root`), and eleven test files now declare `(:use-internals …)`
for the internals they pin. The checker learned the clause
(`setup_check_imports`) and — generally useful beyond privacy — **now
surfaces compile errors as diagnostics** instead of silently swallowing
them. `tests/private_test.blsp` pins the whole contract (block, grant,
alias-resolved block, :only refusal, same-module, top-level hatch);
`namespace_test`'s old "soft privacy" case now pins the hard behavior.
Suite 795/795; `nest check` zero warnings.

## 2026-07-24 — Validation pass, round 3: nested-let hint gap + client-side net leaks

A third adversarial pass, widening from the server to the **client** side of
`std/net` (which the prior two passes under-examined) plus the freshest tooling.
Five real fixes; the reactor kernel bookkeeping itself came back clean.

- **The Clojure/Scheme nested-`let` teaching hint missed its own advertised
  example.** ADR-146's sibling feature (commit 4168ec1) turns `(let ((a 1) (b 2))
  …)` into a "flatten your bindings" hint — but only on the *odd*-length binding
  path. The docstring's own 2-binding example is even-length, so it slipped into
  the compile pass's pattern-lowering (`lower_let`), evaluated `(b 2)` as a call,
  and died with a confusing "unbound symbol: b" — exactly the error the feature
  replaces. Fixed in the compile pass (`macroexpand_all_depth`, so both engines
  report it) and the tree-walker `let`/`letrec` arms. The even-length guard
  (`even_bindings_look_scheme`) requires every value slot to be a *literal atom*,
  so a genuine bare-list destructure (`(let ((a b) '(1 2)) …)`, `((a 1) '(9 1))`)
  is never mis-flagged; letrec (no destructuring) flags the nested shape directly.
  `reader_hints_test` grows 8 → 11.

- **HTTP client fd leak on a stalled peer (fd-exhaustion DoS).** `http-request`
  collected the response then returned — but on the `after`/error branch never
  closed the socket, and the reactor reaps only *unclaimed accepts*, not an active
  claimed stream. A server that accepts then stalls leaked one fd + reactor/registry
  entry per request for the caller's whole life (a long-lived crawler/webhook
  fetcher exhausts its fds). Fixed: `http-request` now `tcp-close`s unconditionally
  after collect (idempotent — the success path's peer-close already reclaimed). The
  client sibling of the 2026-07-23 server head-truncation leak.

- **Unbounded HTTP client response buffer (OOM).** `http--collect` consed chunks
  with no cap; a server streaming an endless body (the silence timeout never fires
  while bytes trickle) grew the accumulator until OOM. The server bounds head+body,
  the client bounded nothing — and it reads *untrusted* output. New
  `*http-max-response-bytes*` (64 MiB); a type-aware `http--chunk-size` measures
  `bytes` (plaintext) and string (TLS) chunks.

- **Unbounded SSE reader buffer (OOM).** `sse--read-head`/`sse--stream` accumulated
  a head with no blank line, or a frame with no `\n\n` boundary, without bound. New
  `*sse-max-buffer-bytes*` (16 MiB) closes the socket and reports
  `[:sse-closed :buffer-overflow]` past it.

- **Fractional/negative `Content-Length` → 0.** `http--declared-length` accepted a
  non-integer length (`3.9`); a fractional target could never be reached exactly,
  parking the reader on the socket until the read timeout. Now only a non-negative
  *integer* is honoured (the timeout already bounds an honestly-shaped lie).

Deferred (noted, not fixed): a reactor idle/handshake timeout so a forgotten
active/`TlsConn` socket is reaped even when the app never closes it — the
defence-in-depth generalisation of the client-close fix, a riskier kernel change
gated on a concrete raw-`tcp`/`tls` consumer (ADR-011). Also left: last-wins
duplicate `Content-Length` (not smuggling-exploitable under HTTP/1.0 `close`) and a
Windows-only backslash path nit in the MCP sandbox (Brood targets Linux). The MCP
write/edit sandbox and the HTTP/SSE parsers were re-audited adversarially and held.

Suite 803/803; `nest check` zero warnings.

## 2026-07-24 — Reactor reap hardening: TLS handshake timeout + opt-in idle timeout

The deferred defence-in-depth item from validation round 3 (the mio reactor's gap:
a stalled or forgotten socket with a live owner is never reaped). Two additions to
`crates/lisp/src/net.rs`, split by what is safe to make a default:

- **TLS handshake-completion timeout (default-on, 30 s).** A peer that opens a TLS
  connection — server-accepted (`tls-listen`) or the client half of `tls-request`
  — then stalls mid-handshake holds an fd the app *cannot* reclaim: it never sees
  the socket until the handshake finishes, so no app-level read timeout can
  intervene. The genuinely reactor-only gap. `TlsConn` carries a
  `handshake_deadline`, set at both creation sites, cleared in `drive_tls` once
  `!is_handshaking()`; `housekeep` reaps a still-handshaking conn past the deadline
  via `tls_finish` (→ `[:tcp-error]` for a client, `[:tcp-closed]` for a server).
  Verified firing at ~2 s with the bound temporarily lowered.

- **Opt-in idle timeout (default-off, `tcp-set-idle-timeout sock ms`).** A blanket
  idle default is impossible: SSE, long-poll, and the editor daemon all hold
  *legitimately* idle connections, and the reactor can't tell "forgotten" from
  "intentionally idle." So it is per-socket opt-in: a server arms it on a
  connection accepting untrusted input (slow-loris protection the reactor applies
  even if the app forgets to close), and everything else is untouched — the
  default-off path never reaps, so a long-idle stream is safe. `PlainConn`/`TlsConn`
  carry `idle` + a `last_activity` stamp; a new `Cmd::SetIdle` arms/disarms;
  `housekeep` reaps an established, armed, idle conn with a terminal message.
  Detection rides the ~1 s housekeep tick (idle bounds are coarse — fine for
  slow-loris). An adversarial review of the reap paths caught two `last_activity`
  gaps, both fixed before landing: the clock must start at **establishment**
  (`Cmd::Claim` / handshake completion), not at accept/arm — else a bound armed
  before the claim, or a slow handshake, reaped a healthy connection the instant it
  came up; and **outbound flush progress** counts as activity too (a large response
  draining to a slow reader was otherwise idle-reaped mid-send). `tests/tcp_test.blsp`
  +3 (armed silent conn reaped; establishment-relative clock; activity resets the
  timer so an active conn is NOT reaped).

The app-side per-read timeout (`*http-read-timeout-ms*`) that already protects the
HTTP server is unchanged. Suite 803/803; `nest check` zero warnings.

## 2026-07-24 — First-class set kernel (`#{…}`, ADR-060)

Promoted sets from the map-backed library to a first-class kernel type — the
long-deferred ADR-060 item. A new `Value::Set(MapId)` shares the CHAMP trie with
maps (`element → true`) so the storage is reused verbatim, but it is its OWN kind
at the value/tag boundary: `set?` true, `map?` false, `type-of` `:set`, prints
`#{…}`, and a set is **never** `=` to a map (even same-keyed).

Paid the full `docs/types.md` compatibility contract for a new `Value` variant:
`Tag::Set` + `ALL_TAGS` bit (the type lattice; without it `ANY` excluded sets and
every `any` param wrongly rejected them — the "expects any, got «blank»" tell),
`value::tag`, the reader (`#{…}` → `Value::Set`, evaluates + dedups its elements),
printer, tree-walker + macroexpander + namespace-resolve arms, structural hash +
`equal` (order-independent, reduces to `map_equal` on the backing trie), a
`ConstVal::Handle` kind, `Message::Set` + the dist wire codec (cross-process
round-trip as a set, not a map), and — the delicate part — the ~dozen
wildcard-guarded GC paths a set shares with maps (copy collector, promote, RUNTIME
compaction flush, `is_movable`/`needs_root_slot`, the `GC_VERIFY` walk, multigen
liveness). The compiler's exhaustiveness caught the explicit matches;
`GC_STRESS`+`GC_VERIFY` covered the wildcard ones.

Sets are **seqable** (`first`/`rest`/`seq_items` yield elements, so
`count`/`map`/`fold`/`into` Just Work; `rest` returns a list so a fold over a set
materialises at most once). Kernel ops `%set`/`%set-add`/`%set-remove`/
`%set-has?`/`%set-count`; `std/set.blsp` is now Brood sugar over them (constructor
+ `conj`/`disj` + `union`/`intersection`/`difference`/`subset?`). Deferred (noted):
compiling a `#{…}` literal in a hot arm to a dedicated `Node::Set`/bytecode — today
such an arm defers to the tree-walker (correct, `_ => None` in `compile_node`); the
VM-eligibility optimization is a follow-up, and quasiquoting into a set literal
(`` `#{~x} ``) doesn't unquote yet.

`tests/set_test.blsp` rewritten for the kernel type (18 tests incl. literals,
`set?`/`map?`, order-independent equality, set≠map, and the `:isolated`
cross-process fan-in); `reader_hints_test` updated (`#{…}` now reads instead of
raising the old teaching hint). Suite **2921/2921**, `nest check` zero warnings,
differential + GC + runtime-collector/multigen green.
