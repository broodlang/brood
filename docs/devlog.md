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
the topic doc (see [../README.md](../README.md) § Documentation) or the relevant `## ADR-NNN` in
[decisions.md](decisions.md). Use the digest to place a change in time; for an early
session's full text, find its `## YYYY-MM-DD — …` header in the archive.

Entries are historical and are not rewritten, so a few older ones link to design
docs since deleted (`concurrency-v2.md`, `supervision.md`, `incremental-check.md` —
mostly trimmed in `fdce540` once their features shipped). Recover from git if needed.

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

- **2026-06-30** — Checker: precise body inference (merely-wider returns) + int-closed arithmetic
- **2026-07-01** — CLI polish + repo hygiene: colored diagnostics, rustfmt gate, CI
- **2026-07-01** — Vectors: inline small-vector storage (closes the `bintree` heap gap)
- **2026-07-01** — JIT back-edge store-elision for carry loops — prototyped, REJECTED
- **2026-07-01** — GC: scale the nursery threshold by *total* live (young+old); rarer majors
- **2026-07-02** — pfib parallel-scaling: kill the inline-upgrade epoch-bump cascade
- **2026-07-02** — unboxed-i64 register calling convention for int-only recursion (SHIPPED)
- **2026-07-02** — remove `let*` (breaking): Brood's `let` is already sequential
- **2026-07-02** — `spit-bytes`: the byte-faithful file write (write side of `slurp-bytes`)
- **2026-07-02** — `image-thumb`: decode + downscale an image to RGBA (inline previews)
- **2026-07-02** — unboxed register worker: f64 sibling (float recursion)
- **2026-07-02** — HOF closure-call fast path in `range_reduce` (modest, and a redirect)
- **2026-07-02** — `todo.md` triage: int `Div` shipped for the unboxed-i64 worker; two items were already done
- **2026-07-03** — HOF native fast-frame: `hof_apply_step` jumps the step arm's native code (nqueens ~18%)
- **2026-07-03** — `%isolate` made RUNTIME-compaction-safe (silent global-misdispatch bug)
- **2026-07-03** — Test-runner memory leak fixed: run each file in its own rolled-back scope
- **2026-07-03** — Audit: two more "off-graph RUNTIME handle across compaction" bugs (declared_sigs, positions)
- **2026-07-03** — Complete the test-runner leak fix: the `nest mcp` structured path too
- **2026-07-03** — Harden the KI-6 fix: compaction-safety moves into snapshot/restore itself
- **2026-07-03** — Harden snapshot/restore against unpaired calls (KI-6 follow-up)
- **2026-07-03** — Scale-test the test runner (100K+ files): two more O(n²) fixes
- **2026-07-03** — Root-caused the scoped-runner quadratic: it was two O(N²) bugs (both fixed)
- **2026-07-03** — check-project O(n²): root-caused, NOT fixed (investigation record)
- **2026-07-03** — Record/shape types: `(record :k T …)` slice 1 (ADR-115)
- **2026-07-03** — Record/shape types: full `fields` refinement (ADR-115, slice 2)
- **2026-07-03** — check-project O(n²): FIXED via header-import redesign (26s → 5.8s @ 4000 files)
- **2026-07-03** — check-project fully LINEAR: the residual was more O(n²) `append`-in-fold sites
- **2026-07-05** — Type-system review (no bugs found) + intersection of arrows (ADR-116)
- **2026-07-05** — Intersection of arrows: cross-module resolution was missing (ADR-116 follow-up)
- **2026-07-05** — Int-literal types: `5` as a type, first slice of ADR-105's deferral (ADR-117)
- **2026-07-05** — `nest check` parallelised across the worker pool (3–4× on huge projects)
- **2026-07-05** — Match exhaustiveness over literal-enum types (ADR-118)
- **2026-07-05** — Bool/string literals, generalized exhaustiveness, match redundancy (ADR-120/121/122)
- **2026-07-05** — Revised direction: pursue full Elixir-parity soundness (ADR-123, design only)
- **2026-07-05** — ADR-123 slice 1: cross-module value-type sigs (ADR-124)
- **2026-07-05** — Merge fallout: ADR-124's new heap read bypassed Phase 2's recorder
- **2026-07-05** — ADR-123's Step 2 turned out to already exist
- **2026-07-05** — ADR-125: `nest run --watch` re-checks on reload
- **2026-07-05** — ADR-126: fixed the defmodule arrow-sig seeding gap
- **2026-07-05** — `nest check --strict` was already built
- **2026-07-05** — ADR-127: `&optional` in `(sig …)` arrow grammar
- **2026-07-05** — ADR-128: tuple / positional product types
- **2026-07-05** — ADR-129: fixed the check-cache staleness bug for real
- **2026-07-06** — checker false-positive sweep (bytes seqable, gensym lint exemption, proc-send)
- **2026-07-06** — Checker: float-contagion arithmetic (the last precise-body-inference slice)
- **2026-07-06** — `nest check` to zero: checker false-positive sweep + `check-allow` directive
- **2026-07-06** — Arrow-intersection argument check (ADR-116 completion)
- **2026-07-06** — Path narrowing: occurrence typing through `(get base :key)`
- **2026-07-06** — Path narrowing, general form: nested keyword-`get` chains
- **2026-07-06** — Path narrowing flows into calls: base record refinement + record disjointness
- **2026-07-07** — Path narrowing: index paths (`nth`/`first`/`second`/`third`)
- **2026-07-07** — Local type inference: the sound (return-only) half
- **2026-07-07** — Gating design + the B0 prerequisite (prototyped, reverted to sound)
- **2026-07-07** — Gating Gap A: undeclared globals get a current-image type
- **2026-07-07** — Gating B0: int/bool/string literal-singleton precision
- **2026-07-07** — Gating B1: argument check through the full gradual relation (Gap B complete)
- **2026-07-07** — Cross-file Gap A (and a dynamic-var soundness fix)
- **2026-07-07** — Fix: stack overflow in Tier-2 return inference (deep bodies) + gate cleanup
- **2026-07-07** — Multi-process RUNTIME GC: Erlang-style 2-generation model (Stages 1a/1b/2)
- **2026-07-08** — Multi-process RUNTIME GC: Stage 3a (the per-process liveness probe)
- **2026-07-08** — Multi-process RUNTIME GC: Stage 3b (the cross-process drain union)
- **2026-07-08** — Multi-process RUNTIME GC: Stage 3c (drain union wired into the scheduler)
- **2026-07-08** — Multi-process RUNTIME GC: Stage 4 (the free mechanism — ArcSwap generations)
- **2026-07-08** — Multi-process RUNTIME GC: Stage 4 auto-arming (live-globals migration + safepoint state machine)
- **2026-07-09** — Sequence API: shrink the lazy surface to `l*` + `->>`, drop transducers/`eduction`
- **2026-07-09** — Multigen RUNTIME GC: diagnosed the suite hang (pre-existing), fix attempts reverted
- **2026-07-09** — Multigen RUNTIME GC: fixed the drain livelock/hang (private self-report walk)
- **2026-07-09** — Multigen RUNTIME GC: closed the throughput gap (two-phase self-report walk) — suite at parity
- **2026-07-09** — JIT is now a default cargo feature (on for everything)
- **2026-07-09** — Remove dead complexity: the inert PoisonBits tripwire + two stdlib dups
- **2026-07-09** — Multigen RUNTIME GC is now unconditional (ADR-091) — flag + dual paths deleted
- **2026-07-09** — brood-life feedback triage: shipped the accepted cluster + ADR-130
- **2026-07-10** — `defrecord` implemented (ADR-130) — pure prelude sugar over maps, zero new core
- **2026-07-10** — Checker now flags a wrong-type `sig` argument at the call site
- **2026-07-10** — Type-checker gating: reconciled docs to the shipped state + reload-aware invariant
- **2026-07-10** — Dead-clause lint broadens to precise surface `let`-locals (ADR-131)
- **2026-07-10** — JIT native-IC increment 2: re-confirmed NO-GO; pivot to leaf inlining
- **2026-07-10** — House-cleaning sweep: seven bugs fixed, one scheduler liveness bug found (deferred)
- **2026-07-10** — House-cleaning, part 2: the deferred bug + all three design calls fixed
- **2026-07-10** — CI back to green + text-mode UTF-8 read-boundary carry (byte-faithful I/O closed)
- **2026-07-10** — Multigen RUNTIME GC: fix the ~300× spawn-scaling regression
- **2026-07-11** — Closure creation caches its parsed template (ping-pong ~7.5%)
- **2026-07-11** — The buffer-process protocol grows up (myedit's actor endgame, both halves)
- **2026-07-12** — The top-level program is a green process (ADR-135): ping-pong 6.5 → 3.3 µs/RT
- **2026-07-12** — Kill an O(n²) landmine in `string->list` (and the `(str acc …)` / `char-at`-scan family)
- **2026-07-13** — Share closure arms behind an `Arc` (`ring` 2.02 → 1.50 s, ping-pong ~18%)
- **2026-07-13** — Unboxed-i64 worker covers tail self-calls: `ackermann` 4.0 → 0.36 s (7/7 → 3/7)
- **2026-07-14** — nbody 6.65 → 1.67 s (~4×): bodies list→vector + variadic MakeVector JIT + selective float carry
- **2026-07-14** — nbody 1.25 → 0.82 s (JIT now earns its keep): fix vector-read + float-handle deopts
- **2026-07-14** — Regex compiles to a lazy DFA (`regex` 1.03 → 0.69 s; catastrophic patterns now linear)
- **2026-07-15** — Table throughput: lock-free registry + fast scalar hash (and why `sieve` stays 7/7)
- **2026-07-15** — Register worker learns `throw`: `errors-deep` 0.28 → 0.07 s (~4×, 5/7 → ~2/7)
- **2026-07-15** — `persistent-map` off 7/7 by transcription; two JIT hypotheses tested and refuted
- **2026-07-15** — Dense Table storage + table-op prims: `sieve` 0.88 → 0.15 s (~6×, at Clojure's heels)
- **2026-07-15** — Regex compiles harder (re:compile discipline) + the JIT learns keyword `=`
- **2026-07-15** — The big one, increment 1: native→builtin calls get an IC fast path
- **2026-07-15** — The big one, rungs 2–4: batch staging, native flat cell, memset frames
- **2026-07-15** — `sqrt` inlines as `fsqrt`: nbody 0.74 → 0.54 s (kills the last coin-flip 7/7)
- **2026-07-15** — `std/json`: the parser goes int-codes, the encoder goes emit-list (0.39 → 0.30 s)
- **2026-07-15** — BEAM-style reduction batching on the JIT loop back-edge (collatz −35%)
- **2026-07-15** — Two profiled cuts: non-exact int `/` inlines (mandelbrot −17%); spilled vectors read through a cached pointer (nbody −9%)
- **2026-07-15** — Refuted: deferred frame stores for register-carried JIT loops (≤2%, reverted)
- **2026-07-15** — Regex's dead `(:use editor/buffer)`: 578 → ~301 ms wall, RSS 182 → 65 MB (one line)
- **2026-07-16** — match/receive lowering was EXPONENTIAL in arm count (editor/buffer load 297 → 7 ms); require's stale in-flight marker (5 s stall on a failed load)
- **2026-07-16** — JIT: closure arms through the call-profitability gate + deopt feedback (nqueens −31%, nbody −28%, pipeline −14%)
- **2026-07-16** — sieve deep-dive: lock-free dense Table + resume-tier fix (sieve −33%, loop −75%)
- **2026-07-16** — JIT inlines dense table ops (sieve 0.10 → 0.06 s, 4/7 → 3/7)
- **2026-07-16** — the stress suite (`make stress`) + KNOWN BUG: JIT deopt re-run duplicates side effects
- **2026-07-16** — FIXED: JIT deopts resume at effect-safe checkpoints (the deopt-rerun bug)
- **2026-07-16** — stress suite grows external-style batteries (R7RS- and Clojure-inspired)
- **2026-07-16** — conj lands; the stress kit gains a program fuzzer + chaos preemption
- **2026-07-16** — BROOD_VM=0 honored again at top level; the fuzzer gets its third oracle
- **2026-07-16** — float printing goes shortest-round-trip; a reader/printer round-trip battery; an honest false alarm
- **2026-07-16** — stress kit round 3: concurrency fuzzing, formatter properties, checker soundness
- **2026-07-16** — TSAN clean, loom model-check, fuzzer auto-shrink
- **2026-07-16** — Auto-shrink pays off: JIT sibling-`let` slot-reuse miscompile
- **2026-07-16** — Coverage-guided fuzzing finds a second bug: VM error-format divergence
- **2026-07-16** — ASAN pass: kernel is memory-clean; i64-path fuzz edges
- **2026-07-16** — Reader/evaluator robustness fuzzer (adversarial input)
- **2026-07-17** — Chasing mod.rs coverage: two optimization passes now fuzzed
- **2026-07-17** — The targeted pass happened anyway: HOF driver + deopt/effect shapes
- **2026-07-17** — Checker: a file's own defn now supersedes a builtin's signature
- **2026-07-17** — string->codepoints: the missing text-access primitive
- **2026-07-17** — spawn regression root-caused: the shared-arm compile flood
- **2026-07-17** — Inlined-upgrade queue gets the same flood dedupe
- **2026-07-17** — regex leaves 7/7: cache split, vector hot-object, and a deopt storm
- **2026-07-18** — bintree: the checkpoint tax measured honestly, and a purity exemption
- **2026-07-18** — Docs brought back to 100%: the full staleness sweep
- **2026-07-18** — Stack traces in error values (BEAM/.NET gap #1 closed)
- **2026-07-18** — Per-process heap limits: `(process-flag :max-heap n)` (survey gap #2, lever 1)
- **2026-07-18** — Death reasons carry the structured error (trace follow-up)
- **2026-07-18** — Link propagation carries the originating reason (survey housekeeping)
- **2026-07-18** — Dirty-CPU accounting: long natives charge reductions + named stalls
- **2026-07-18** — Dist self-healing: net/reconnect + opt-in :send-errors (survey gap #5)
- **2026-07-18** — Review pass over the day's work: ensure-link consolidation + fixes
- **2026-07-18** — Benchmark regression sweep over the day's work
- **2026-07-18** — Feature-parity push: Erlang timers land; two "missing" OTP items were already shipped
- **2026-07-18** — Observability timing tier, slice 1: GC pauses, sched counters, a sampling profiler
- **2026-07-19** — Observability slice 2: the kernel event stream (`system-monitor` → telemetry)
- **2026-07-19** — Cold-start measured: it's macro expansion (27 of 31 ms), not eval
- **2026-07-19** — Fix: tree-walked frames are native for capture purposes (TreeWalkGuard)
- **2026-07-19** — Boot cache shipped: ~38 ms → ~6.5 ms cold start (ADR-138)
- **2026-07-19** — Leaf-callee inlining behind BROOD_JIT_LEAF_INLINE (~30% on helper-loop shape)
- **2026-07-19** — Mailbox receive: one lock per matched message (was three)
- **2026-07-19** — `type-of` as `PrimOp1::TypeOf`; a type-mixed-join JIT miscompile found and fixed
- **2026-07-19** — The no-JIT build compiles again (and CI now keeps it honest)
- **2026-07-19** — Leaf-callee inlining flipped to default ON (`BROOD_NO_LEAF_INLINE` opts out)
- **2026-07-19** — Iolists at the write boundary (ADR-139)
- **2026-07-19** — Iolists follow-up: the deep-nesting test found a real kernel limit
- **2026-07-19** — std/net/http on iolists; a real Content-Length bug fixed
- **2026-07-20** — Deep-value stack safety: segmented growth in the recursive heap walkers
- **2026-07-22** — Bit syntax: typed integer segments in the bytes pattern (ADR-140)
- **2026-07-22** — The parser port: std/net bytes-native, carrier strings deleted (ADR-141)
- **2026-07-22** — Identity: general-purpose language; README/CLAUDE.md/ROADMAP reframed
- **2026-07-22** — Tier 1 items 2+3: no read-buffer transient (ADR-142); the socket reactor (ADR-143)
- **2026-07-22** — Tier 1 item 4: the dirty-native offload pool (ADR-144) — Tier 1 complete
- **2026-07-22** — The freeze wart fixed: reachability-aware dangling-env check
- **2026-07-22** — Validation pass over the day's kernel changes
- **2026-07-22** — WASM component interop, slice 1 (ADR-145): sandboxed native extensions
- **2026-07-23** — Finish-the-partials batch 1: embedded teardown, checker hardening, fuzz targets, and a repo-wide build bug
- **2026-07-23** — Clojure/Scheme teaching hints (reader-level)
- **2026-07-23** — Validation pass, round 2: a remotely-triggerable server crash + 3 more
- **2026-07-23** — Adversarial validation pass over the day's work: 7 real fixes
- **2026-07-23** — MCP streaming/progress tier
- **2026-07-23** — LLM-native MCP tools: explain-error + find-pattern
- **2026-07-23** — Finer type/arity finding spans (LSP/`nest check`)
- **2026-07-23** — Symlink-escape-proof MCP write sandbox (`canonicalize`)
- **2026-07-23** — `nest format --changed`
- **2026-07-23** — "Private should be private": module privacy enforced (ADR-146)
- **2026-07-24** — Validation pass, round 3: nested-let hint gap + client-side net leaks
- **2026-07-24** — Reactor reap hardening: TLS handshake timeout + opt-in idle timeout
- **2026-07-24** — First-class set kernel (`#{…}`, ADR-060)
- **2026-07-24** — WASM interop slice 2: bytes marshalling (`list<u8>` ↔ `bytes`)
- **2026-07-24** — LSP tier-3: incremental document sync
- **2026-07-24** — Telemetry metric aggregators + sampling (Elixir Telemetry.Metrics, in Brood)
- **2026-07-24** — LLM-native / MCP polish: watch-runtime trace tool + cookbook entries
- **2026-07-24** — Finish-the-partials: reader hints (`#_`/`#"…"`/`\c`) + telemetry histogram + node-liveness stream
- **2026-07-24** — Package manager v2: tarball deps + a git-backed registry (ADR-147)
- **2026-07-24** — File-organization pass: split the giants, no behavior change
- **2026-07-24** — Structural cleanup Tier 3 (quick wins) + a broken-build fix
- **2026-07-24** — `:format-plugins` now resolves any dep kind, not just `:path`
- **2026-07-24** — Structural cleanup Tier 2 (dedup)
- **2026-07-24** — Structural cleanup Tier 1 item 4: PRIMITIVE_DOCS drift guard
- **2026-07-24** — `nest test` selection: `mix test` parity
- **2026-07-24** — review pass on `nest test` selection: four defects fixed
- **2026-07-24** — Structural cleanup Tier 1 item 2 (partial): scheduler guards split
- **2026-07-24** — `nest test --cover`: function coverage with zero kernel support (ADR-148)
- **2026-07-24** — Structural cleanup Tier 1 item 1 (partial): extract jit_lower/i64.rs
- **2026-07-24** — hardening pass over the whole `nest` surface
- **2026-07-24** — Tier 1 item 2 continued: extract scheduler/lifecycle.rs
- **2026-07-24** — Tier 1 item 2 complete: extract scheduler/pool.rs
- **2026-07-24** — Tier 1 item 1 cont.: start jit_lower_arm_inner decomposition (prepass)
- **2026-07-24** — adversarial pass over `nest`: five real bugs, two of them serious
- **2026-07-24** — Tier 1 item 1 cont.: Op → module scope + extract jit_lower/emit.rs
- **2026-07-24** — Tier 1 item 1 cont.: extract scalar slot helpers (Frame context)
- **2026-07-24** — Tier 1 item 1 cont.: extract slot-kind tracking helpers
- **2026-07-25** — jit_lower decomposition: roadmap the remainder + a heavier test pass
- **2026-07-24** — `nest completions`: project-aware TAB completion
- **2026-07-25** — a warning-free test run, and the flaky test I introduced
- **2026-07-25** — `nest new` produced projects that failed their own toolchain
- **2026-07-25** — sweep of the remaining `nest` commands
- **2026-07-25** — completion follow-ups: two wrong-value bugs, and real latency numbers
- **2026-07-25** — a missing-file inconsistency, and the suite's one un-retried flake
- **2026-07-25** — two more leaked-internals fixes, and the last untested commands
- **2026-07-25** — CLAUDE.md drift audit
- **2026-07-25** — concurrency probe: manifest edits are not safe, and saying so honestly
- **2026-07-25** — Tier 4 io.rs split + jit_lower Batch 5 / Funcs / big helpers
- **2026-07-25** — fixed the manifest race (`%file-swap`)
- **2026-07-25** — jit_lower emit-loop decomposition: the per-`Inst` arm bodies (COMPLETE)
- **2026-07-25** — types: sound parameter inference from unconditional demands
- **2026-07-25** — framed reads: the input-side twin of iolists (tcp-read-until / -n)
- **2026-07-25** — `nest test` output: dots by default, an informative coloured trace, and `:skip`
- **2026-07-25** — making a `nest test` failure readable
- **2026-07-25** — clearing the known-issues list: three fixed, one reframed, one honestly deferred
- **2026-07-25** — line coverage, third attempt: the denominator was the whole problem
- **2026-07-25** — KI-10 (the `receive` 13-arm cliff) no longer reproduces
- **2026-07-25** — the sibling projects: two fully broken, both fixed; one left to decide
- **2026-07-25** — external conformance corpora, suite 1: `parse-number-fxx`
- **2026-07-25** — conformance corpora, suite 2: `dectest` finds two real decimal bugs
- **2026-07-25** — conformance corpora, suite 3: JSONTestSuite finds an RFC bug and KI-11
- **2026-07-25** — syntax finalisation: seven places the surface reinterpreted instead of rejecting
- **2026-07-25** — conformance corpora, suite 4: UCD, and two new Unicode primitives
- **2026-07-25** — conformance corpora, suite 5: csv-spectrum, and three suites ruled out
- **2026-07-26** — KI-11 fixed, and three more corpora (one found a `pow` bug)
- **2026-07-26** — `sig` adoption (the pilot that broke four things), and the alias trims
- **2026-07-26** — KI-12: the prelude froze a RUNTIME handle as PRELUDE (and `:conformance` now buys budget)
- **2026-07-26** — the syntax finalization, from downstream: three brood defects it exposed
- **2026-07-26** — ergonomics & conciseness pass (ADR-154): add the sugar, cut the surface
- **2026-07-26** — conformance corpora, suite 13: the Gabriel benchmarks, and a hole in the engine gate
- **2026-07-26** — the ADR-154 rename, from downstream: what a mechanical rename gets wrong
- **2026-07-26** — the message rows: `receive` clause bodies move into the calling function (ADR-155)
- **2026-07-26** — syntax review, part 2: the collection protocol, and two patterns that lied (ADR-156)
- **2026-07-26** — `:else` cost 12×, and the fix is a constant-test fold (ADR-157)
- **2026-07-26** — the hint table was lying in five places
- **2026-07-26** — the syntax review's remainder: protocols, graphemes, patterns, transducers (ADR-158…163)
- **2026-07-27** — documentation run: `ability` reaches the docs, and validation found four real bugs
- **2026-07-27** — KI-14 root-caused and fixed: `make test` goes green
- **2026-07-27** — published benchmark run: KI-14's stack guard costs a call
- **2026-07-27** — reader reservations (ADR-169): the last pre-freeze language decision
- **2026-07-27** — type-checker gate cleanup (93 → 8 gating) + ADR-170 freeze list
- **2026-07-28** — tree-walker use-after-GC on `(runtime-collect)` in an `&optional` default
- **2026-07-28** — the GC's forwarding tables were hash maps; `sort` −17.6%
- **2026-07-28** — REPL completion: list the candidates when Tab can't extend
- **2026-07-28** — REPL: bounded printing, result history, rc file, auto-indent + M-q
- **2026-07-28** — per-process compiled code is the per-process memory cost
- **2026-07-29** — ADR-175 Phases A+B land: shared compiled code, spawn-live −37%
- **2026-07-29** — `std/` adopts abilities across the tree (ADR-177)
- **2026-07-30** — sealed-match exhaustiveness (ADR-187 part 2)
- **2026-07-30** — occurrence typing: inferred params check callers (ADR-190)
- **2026-07-31** — the overnight soak, and why nine hours does not fit
- **2026-07-31** — soak result: 12.7M iterations clean, and an 8× throughput decay a restart cures
- **2026-07-31** — the CI type-checker gate reaches zero: 60 warnings fixed or justified
- **2026-08-01** — std/ scale sweep: `write-lines` was O(total²), and so was `nest doc`
- **2026-08-01** — KI-24: `eval` lost forward references (a regression in the eval-on-the-VM change)
- **2026-08-02** — thread 6: both cheap mitigations ruled out at code level, with a fresh baseline
- **2026-08-02** — scale sweep: `template/render` was a real quadratic (318 ms → 24 ms)
- **2026-08-02** — reverse string search was quadratic on an editor hot path (540 ms → 1 ms)
- **2026-08-02** — `strip-ansi` was two stacked quadratics (1583 ms → 109 ms)
- **2026-08-02** — `stream-lines` was quadratic inside a chunk (303 ms → 39 ms), and the prediction held
- **2026-08-02** — Namespacing hardening (audit follow-up)
- **2026-08-02** — correcting myself: `format-source` WAS superlinear (3.6 s → 2.0 s)
- **2026-08-02** — the last quadratic row: a cached char count, and `expect_string`'s hidden copy
- **2026-08-02** — every string builtin cost O(argument length), whatever work it did
- **2026-08-02** — correcting the claims the measurements contradicted
- **2026-08-02** — thread 2 profiled: the 27 µs is spawn PLACEMENT, not per-message cost
- **2026-08-02** — option (b): wake an idle peer to steal a fresh child, with first refusal
- **2026-08-02** — recovering the `supervisor` 11%: tell the two spawner shapes apart by history
- **2026-08-02** — published the cross-language run, and the regression that wasn't
- **2026-08-03** — soak on the scheduler change: 3.18M iterations, zero violations
- **2026-08-03** — verification pass on the reformatted tree (and a full disk)
- **2026-08-03** — thread 6 fixed: a shared closure crosses a serialised send by handle
- **2026-08-03** — thread 6 validated, and where its win does *not* show
- **2026-08-03** — thread 6b closed by thread 6, not deferred
- **2026-08-03** — the per-process floor, attributed (it was "roughly half unattributed")
- **2026-08-04** — three loose ends from ADR-210, and why the flake fix is not a retry
- **2026-08-04** — resolver: full multi-requirer derivations + pre-release ordering (ADR-209)
- **2026-08-05** — `spawn-live` again: the lever was `fold` over a vector, and my §1 premise was wrong
- **2026-08-05** — privacy moves off the name onto the def form: `defn-`/`def-` (ADR-146 step 2)
- **2026-08-06** — the receive matcher never reaches the native fast frame
- **2026-08-06** — release 0.3.0, and a `CHANGELOG.md`
- **2026-08-06** — semver ranges over `:git` tags: the resolver's last deferred item
- **2026-08-06** — bug hunt: a version range from another ecosystem resolved wide
- **2026-08-06** — what LFE has to teach us: a guard-purity lint + two doc corrections
- **2026-08-06** — Stage 6: a userland `code_change` for `gen` servers
- **2026-08-06** — REPL package-rooting: the interactive `%in-ns` + the checker to match
- **2026-08-06** — Module-load scaling: `*features*` was O(n²); where the other two walls are
- **2026-08-06** — the JIT re-tiered every arm on every `def` (ADR-217)
- **2026-08-06** — source positions: an allocation bug, a GC re-key, and what's actually left
- **2026-08-06** — what it would take to start a 10×-moneyclub project: measured against Elixir
- **2026-08-06** — the startup image ships (ADR-218): 30.6 s → 8.1 s on a 16 300-file project
- **2026-08-06** — image follow-up: a lost sig, my own quadratic, and 4 s of pointless compaction
- **2026-08-06** — the image goes lazy: 16 300 modules start in 1.3 s and 219 MB
- **2026-08-07** — `nest run` on 16 300 files: 127 s → 1.2 s
- **2026-08-07** — KI-33: fully consuming a stream leaked its producer process
- **2026-08-07** — image audit: seven missing registries, and a cache that never pruned
- **2026-08-07** — the startup image was never read from, and nothing failed
- **2026-08-07** — `tool/sexp`'s forward window scan goes native (`scan-form-end`), the last interpreted loop in a keystroke motion
- **2026-08-07** — the image build wipes a pre-build runtime rebinding (a wipe, not a leak) — contract, no fix
- **2026-08-07** — dependencies get imaged too, once their files are in the staleness key
- **2026-08-07** — a builtin and a byte literal can be imaged; a std module's cache table is not imaged at all
- **2026-08-07** — `nest run`'s cold pre-flight was quadratic: 26.5 GB → ~2 GB
- **2026-08-07** — KI-37: an imaged start never followed a module's require edges
- **2026-08-07** — `nest run`'s cold pre-flight checks the entry closure, not everything loaded
- **2026-08-07** — three gaps hatch found: table globals vs the image, unbounded framed reads, `nest format`'s scope
- **2026-08-08** — an optional compression level for the zlib encoders
- **2026-08-08** — brotli compression (`Content-Encoding: br`)
- **2026-08-11** — standard-library module names are reserved package names (ADR-220)
- **2026-08-11** — the backend seam: a `JitBackend` contract, and the decisions hoisted above it
- **2026-08-12** — the forward-ref pre-scan was not module-boundary-aware
- **2026-08-12** — multiple modules per file: the region model (ADR-223 Phase 1)
- **2026-08-24** — a primitive's name gets one definition site; CST-backed `nest rename` (ADR-240)
- **2026-08-24** — a library name may shadow core only when used qualified (ADR-241)
- **2026-08-24** — the bare core: 613 published names down to 337 (ADR-242)
- **2026-08-24** — the namespace waves: 337 bare core names down to 291
- **2026-08-26** — KI-61 fixed: the prelude autoloads instead of force-loading; boot 22.8 -> 11.6 ms (ADR-246/247)
- **2026-08-26** — KI-64 fixed: a JIT block-argument spill was landing on the deopt journal (ADR-248)
- **2026-08-26** — the codecs: `json` parse 1.8x (row -20.8%) and `base64` decode 1.8x (row -9.5%) (ADR-249)
- **2026-08-26** — every package's `:brood` floor was a lie; the ecosystem release train that fixed it
- **2026-08-27** — migrating the ecosystem across the waves; two outages, both from verifying the wrong artifact (KI-66/67)
- **2026-08-27** — stdlib cleanup: `name` folds into `->string` (ADR-258); the seq orphans go home; `last-index-of` moves to `string/`
- **2026-08-28** — the capture hazard behind that rename: prelude templates, and finishing the `/name` root escape (KI-73)
- **2026-08-27** — the type system, audited then rebuilt: `sig` fails closed and the definition owns the arity (ADR-259), the walk's totality is gated and found the quasiquote gap (ADR-260), a parameter's type is its domain (ADR-261), a union keeps its terms (ADR-262), `(not T)` (ADR-263)
- **2026-08-28** — a record is closed, and openness is the type of the keys it doesn't declare (ADR-264) — which is what makes ADR-262's tagged union usable rather than merely representable

---

## Recent — full entries

The last day or two in full; older sessions are condensed into the digest above,
their full text in [devlog-archive.md](archive/devlog-archive.md) (and git history).
Append new sessions below (newest last).

## 2026-08-13 — ADR-223 Phase 2 (MVP): cross-file require-by-name for a co-located module

Made a co-located *secondary* module reachable from another file by its own name — the piece
Phase 1 deliberately left out. The design agent's finding shrank this from the "unified index"
framing to **one generalization plus a consolidation**: the file→module scan
(`package/package-module-files`, `std/tool/package.blsp`) recorded only each file's *first*
`(defmodule …)` via a `read-first` helper; it now records **every** declared module. That
single change flows unchanged through the existing rooting call sites
(`project-root-project-rooting` for the root project, `package-register-rooting` for deps),
which already register `pkg/mod → file` into `*package-module-files*` — so a co-located
secondary gets `pkg/secondary → file` for free, and `require-force-in`'s existing
`*package-module-files*` branch loads it through `require-force-package`. **No prelude change,
no new require branch, no new Rust builtin.** The key scheme cannot drift because
`(:use secondary)` expands to `(require (%root-module-name 'secondary))` and `%root-module-name`
roots iff `secondary` is in the active package's module set — which the generalized scan is now
what puts there; registration key and lookup key are the same rule over the same set.

Done as a **consolidation, not an addition** (the request was explicitly to clean up, not just
add): the singular `package-module-name-of` was deleted once `package-module-files` and
`package-provided-modules` moved to the new plural `package-module-names-of`, and their
redundant `filter … nil?` guards went with it — so the tree has *fewer* file scanners than
before the feature. `package-provided-modules` was generalized in lockstep so the ADR-070
collision guard still rejects two files declaring the same module name (now including a
secondary). `project-dep-module-files` gained a `distinct` (a two-module dep file otherwise
double-counts in the image fingerprint). Observed but deliberately **not touched** (out of the
feature's blast radius, subtly different, and a green tree not to disturb mid-feature): the same
"first defmodule of a file" logic is reimplemented in `project-file-feature` (rooted, for
load-dedup) and twice inline in `std/tool/docs.blsp` — a worthwhile *separate* consolidation
with its own tests.

**Scope:** named (ADR-070 rooted) projects only. A nameless project skips rooting and a bare
`brood file.blsp` run does no project setup; supporting either needs a new bare-key registry
*and* a new require branch — deferred as not worth the surface for the marginal case. The
checker needs nothing: `nest check`'s whole-project pre-flight loads every source file. The
persisted AOT index (module↔file decoupling, LSP routing, folding the checks into an index)
remains deferred to M2. Six new `:isolated` cases in `tests/project_test.blsp` pin it (the scan
surfaces both modules; the secondary registers rooted; the ordering case — require the secondary
before its file loads; a consumer `(:use)`s it across files; two files declaring one name still
collide; concurrent require loads the file exactly once). This is the substrate for the
requested co-located **test-module** feature — a `foo-test` module beside `foo`, run by
`nest test`, stripped in the AOT/bundle step (next).

## 2026-08-13 — closing out: the whole-tree format, and KI-39's local avenue exhausted

**`nest format` across the tree — 356 files considered, 23 rewritten, `--check` now clean.** This
was the backlog the handoff had recorded as deliberately untouched (12 files when that note was
written, 23 by now). Nine are embedded `std/` modules, including `prelude.blsp` and `format.blsp`
formatting its own source, so it was verified with **both engines** rather than one:
`make test-both` 978/978 + 978/978. That the formatter is idempotent on itself (a second
`--check` pass is clean) is the other thing worth having checked rather than assumed.

A merge landed mid-push (`f025d56d`'s forward-ref fix, ADR-223), and its new cases in
`tests/namespace_test.blsp` arrived unformatted into a file this branch had just reformatted —
`format --check` went red *after* a conflict-free merge. Caught by re-running the check rather
than reading "no conflicts" as "clean tree".

**KI-39 — the local hunt is over, and it found nothing: 0 failures in 15 runs.** Fifteen runs of
the *single* most faithful shape (`BROOD_VM=0`, `taskset -c 0-3` **and** `-j 4`, prelude cache
colded each iteration), 978/978 each, 978–1015 s, a 3.7% spread with no outlier. Deliberately not
"one run each of several configurations" — that is what the earlier attempt did, and against a
27% flake six such runs have a **14.8%** chance of seeing nothing, so its negative result was much
weaker than it read. Fifteen puts that at 0.8%.

**The question worth recording is whether the bug is still there at all, and the answer is that we
do not know.** Four green CI runs since the last failure do not indicate a fix: at 27%, four
consecutive greens are **28% likely anyway**. "Fixed" and "still present" are both unfalsified.
Fixed-in-passing is the weakest of the three readings — the last failure (`79e7e555`) came *after*
the registry and tls fixes landed in `14b1db40` — leaving "still present, we were lucky" and
"runner-dependent" as the live ones. Calling it gone wants ~10 consecutive green CI runs (4%),
which accumulate for free.

Also killed by measurement, so it is not re-derived: the cold-boot-herd hypothesis (that the
~50 unwarmed test binaries pay a *tree-walked* prelude expansion and reproduce KI-38's herd in the
one job that flakes). Cold boot is **1213 ms** at the default ceiling against **1274 ms** under
`BROOD_VM=0` — expansion does not go through the selected engine, and the cache key is
engine-independent, which is why one file serves both.

What remains is instrumentation rather than a hunt: `2312d4a1` makes the failing cases self-report
as **check-run annotations**, which are readable with plain repo access where a run's log needs
admin (403) and a rerun needs write (401). Validating that parser against a real two-failure log
caught a bug in it — the first regex expected `file:line:` and silently missed
`registry_test.blsp:55:9`, i.e. it would have annotated *half* the failures, which is worse than
none because an incomplete list gets trusted.

## 2026-08-13 — `nest check` (and `brood`) no longer SIGABRT on a broken pipe

**Bug:** `nest check … | head` (or a quit pager, or a scrolled-away TUI) aborted the process with
`failed printing to stderr: Broken pipe (os error 32)` and wrote a `.brood_crash_dump` — normal
Unix pipe teardown masquerading as a hard crash. Rust installs `SIG_IGN` for SIGPIPE, so a closed
reader surfaces as an `EPIPE` *write error*, and the `eprintln!`/`println!` macros **panic** on it;
that panic then fired the crash-dump hook. Every `Broken pipe` entry in `.brood_crash_dump` bottomed
out here. It was **half-fixed already**: `builtins::io::write_stdout` had been hardened (devlog
note in its doc comment), but the stderr paths and the CLI's own print sites had not.

**Fix (output plumbing only, 3 files):** `cli_support::report_error` builds its message once and
writes it through a broken-pipe-safe `write_stderr` (exposed as `pub` alongside a `write_stdout`
twin — on `EPIPE` exit quietly like the default SIGPIPE disposition, drop any other write error);
`builtins/io.rs` grows a `write_stderr` mirroring `write_stdout` and routes `eprint`/`%write-err`
through it; and `crates/cli/src/main.rs`'s streaming warning sink (`check_one_file` — the exact
crashing frame) uses the safe writers for both sinks. Reproduced the abort (exit 134 + fresh dump),
confirmed gone for `nest check`, `brood <file>`, and `brood --check` (exit 0, no dump), normal full
output byte-for-byte unchanged; `make test` green (980/980).

**Deliberately not** a global SIGPIPE→`SIG_DFL` reset (the other common CLI fix): std's dist/net
sockets rely on `SIG_IGN`+`EPIPE` to survive a peer disconnecting mid-write, so a global reset
could kill a running node.

## 2026-08-13 — the VM's call path was contending on one cache line (ADR-224), and the gates that should have caught it

Started as "look at the VM vs JIT slowdown" and became three things: a real contention fix, an
audit of the measurement tooling that had hidden it, and the discovery that the breakage suite
had been red for months.

**The fix (ADR-224, KI-40).** `pfib` (100 × `fib(32)`) at ceiling 1 ran **54.4 s with the cores
stalled at 769%** — not saturated, stalled. Twelve independent OS processes running the same
work inflate only 2.5× (SMT + all-core clock), so that was the floor and the rest was
contention. A 2×2 isolated it: same-arm × sharing-on was the *only* slow cell (3881 ms/task
against 1925/1992/2046 for the other three), so the cost needed both a shared arm object and
several threads touching it. `BROOD_GC_FLOOR=2000000` made it *worse*, ruling out GC frequency.

The mechanism is ADR-175 Phase B meeting the VM's call path: a shared arm lives in one
allocation, and the path clones that `Arc` **three times per call** (the IC probe, the
`BcFrame`, and `live_arm_push` for a handle-bearing arm), so N workers RMW one refcount cache
line per call. `vm_call_ic_fast_link` already documents this exact cost — *"the one real
atomic-RMW the hot recursive call (`fib` &c.) otherwise pays per call (~30M times)"* — and
already avoids it, but it is `#[cfg(feature = "jit")]` and serves native-to-native links only.
That is precisely why tier 2 is immune and the VM is not.

`ArmHandle` is a process-local `Arc` owning the shared one, created once per (process, call
site) at IC-fill. **54.4 s → 17.1 s (3.19×)**, at parity with `BROOD_NO_SHARED_ARMS` — the
contention is gone, not reduced. The immortality route (`Owned | Immortal` across ~42 sites)
was rejected: sound for a sealed PRELUDE arm, *not* for a RUNTIME one that
`shared_closures_clear` invalidates. The handle needs no such argument — it holds the shared
`Arc` at least as long as a direct clone would, so liveness is strictly stronger and no GC
invariant moves. Cost: `spawn-live` **+1.8%**, reproduced twice at best-of-21, which is real and
mechanistic (per-process work is exactly what ADR-175 exists to avoid) and recorded rather than
rounded away — it is under `ab-bench`'s 5% gate.

**Why no existing gate would have caught it.** The VM's answer is correct either way, so `make
test`, `make test-both`, the JIT differential and the lowering witness all stay green and *only
a benchmark moves* — the ADR-221 blind spot again. And the benchmark wouldn't have either:
`make ab` measures the **default ceiling**, where a hot arm is native and the interpreter's call
path never executes, so it reported the 3.19× regression as **+1.3%**. Hence
`arm_handle_clone_does_not_touch_the_shared_arm_refcount` (asserts on the shared refcount,
sabotage-verified at 1002 against 2) and **`make ab-vm`** (ceiling 1).

**The tooling audit.** Four traps, each of which produced a *plausible* table rather than an
obviously broken one, now written into `docs/benchmarking.md` §1:

- `timeout(1)` rounds wall up to a **100 ms grid** (78 ms → 104, 103 → 204), so every
  sub-second row reads as a multiple of 100 and every delta as `+0.0%`. `ab-bench.sh` already
  knew this in a comment; nothing else did, and a hand-rolled sweep re-learned it.
- The installed `brood` predated ADR-222, so `BROOD_TIER` was a **silent no-op** and the first
  sweep read `1.0×` on all 23 rows — which reads as a finding about the tiers rather than a
  mistake about the binary. `--version` now prints the build sha (`cli_support::VERSION_LINE`)
  and **`make doctor`** diffs it against HEAD, along with strays, boot-cache state and litter.
- `jit-lower-witness.sh` defaulted `BROOD` to `target/release/brood` while `make release-brood`
  writes `target/release-fast/` — measuring a stale binary reproduces the baseline set, so the
  diff comes back **empty** and the restructuring looks proven. Fixed, plus a staleness warning.
- `ab-bench.sh`'s `parallel_rows` omitted **`spawn-live`** and `supervisor`, so the two rows
  whose whole point is holding 300 000 / 20 000 live processes were being A/B'd pinned to one
  CPU. Added; `--tier` added alongside.

**KI-42 — the breakage suite had rotted to 9 of 23 files red**, all pre-existing (verified
against a baseline build), none about the JIT/VM/memory it exists to stress: a pin-syntax change
(`~ref` → `^ref`, 9 sites), a renamed `string-contains?`, and `(assert= 4.8 (/ 24 5))` predating
exact rationals — that last one had failed on every build since 2026-06-10. Seven fixed; two
skipped by name in `BREAKAGE_SKIP` because their fixes change what the tests measure. It rotted
because it is outside `make test` and had **no runner at all**; there is now a `breakage` CI job
on main pushes. Worth remembering how it was first mis-scoped: reading the suite output through
`tail` shows the *last* failure and reads convincingly as "one broken assertion".

**Gates:** `make test` 981/981, `make test-both` 980+980, jit 40/40 (and 40/40 under
`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`), the GC_STRESS sweep over the seven concurrency binaries
**37/37** (matching `afe4bcff`), tsan and loom green, both compaction guards, the fuzz
differential, `--no-default-features`, clippy, rustfmt, `perf_test.blsp` 16/16, and the lowering
witness byte-identical at 94 fingerprints.

## 2026-08-13 — `reduce-while`: the early-terminating fold (roadmap: Elixir-loved ergonomics)

Added `reduce-while` — `(reduce-while f init coll)` where `(f acc x)` returns `[:cont acc']` to
continue or `[:halt acc']` to stop, returning the last accumulator (≈ `Enum.reduce_while`). A
pure prelude fn (`std/prelude.blsp`, next to `take-while`/`drop-while`) over `seq` + a
tail-recursive `reduce-while-loop` accumulator and a `match` on the reducer's `[:cont|:halt v]`
result; a return that is neither raises. No new special form, no kernel change — the dogfood
cadence (a combinator we lacked, written in the language). Tests in
`tests/prelude_enum_test.blsp` (8 cases incl. halt/all-cont-is-fold/empty/map-pairs/find-first/
bad-return, plus an `:isolated` cross-process case proving the reduced value round-trips through
`send`). `docs/language.md` + `ROADMAP.md` updated.

## 2026-08-13 — require-by-name for a co-located module in a nameless project / bare run (ADR-223 Phase 2b)

Lifted the named-project scoping on ADR-223 Phase 2. A module declared beside another in one
file (region model), whose name is not its file's, was reachable by name only in a *named*
project (whose rooting scan fills `*package-module-files*`). A nameless project or a bare
`brood file.blsp` run fell through to the `<name>.blsp` filename probe and errored. Fixed with
**one fallback** in `require-force-in`'s existing `else` branch (`std/prelude.blsp`): on a probe
miss, scan `*load-path*` for the `.blsp` declaring `(defmodule <name> …)` and load it
(`require-find-colocated` → `require-colocated-files` → `require-file-declares?`, a read-only
`read-all` parse). Fires only after the probe misses — the common path is untouched; two files
declaring one name error loudly. No new registry and no reordering of the resolved branches (the
original scoping had anticipated needing both), and one code path covers both cases since it
reads `*load-path*` (source dirs for a nameless project, the script dir for a bare run) rather
than project state. Verified end-to-end (nameless project via `nest run`, bare run, ambiguity
error, filename-match regression); in-suite tests in `tests/namespace_test.blsp` (`:isolated`, incl.
a cross-process case). `docs/decisions.md` (ADR-223 Phase 2b) + `docs/module-index.md` updated.

## 2026-08-13 — Function-head guards: `:when` on defn/fn clause heads (ADR-226)

A `defn`/`fn` clause head may now carry a `:when` guard — `((n) :when (< n 0) :negative)` —
with `match`'s semantics (clause applies iff the guard is truthy, else fall through). It
already worked in `match`, refutable `let`, and *pattern* clauses; an *arity-only* clause
silently ignored it (the native argument-count dispatch has no place to run a guard).

Met "keep the core small" by **reuse, not new machinery**. `eval/macros.rs` already has two
paths: arity-only fns compile to native `ClosureArm`s; any pattern clause lowers the whole fn
to the `match*` engine, which *already* evaluates `:when` (fn clauses are match* clauses,
passed verbatim). So a new `clause_has_when_guard` makes a `:when` clause no longer count as
"arity-only" in `lower_fn`/`fn_needs_lowering`, routing the fn through `match*` — guard eval,
fall-through, hygiene, name resolution, and **TCO** all come for free (verified: a guarded
`count-down 200000` does not overflow). Cost is opt-in (only a guarded fn pays match-dispatch).
`:when` + `&optional`/`&` in one clause is a loud "guard-dispatched fn" error (one mechanism
per fn). Tests in `tests/pattern_matching_test.blsp` (8 cases incl. mixed arities, precondition,
TCO, macro-hygiene via the existing paths, and an `:isolated` cross-process case); `nest check`
clean on guarded fns. `docs/language.md` + `ROADMAP.md` + ADR-226 in `docs/decisions.md`.

## 2026-08-13 — `tap` / `then`: the single-function pipe helpers (Elixir Kernel parity)

Added `(tap x f)` — call `(f x)` for its side effects and return `x` unchanged (a pass-through
probe in a `->` pipeline) — and `(then x f)` — return `(f x)`, so a `->` step can call an
arbitrary function on the threaded value instead of splicing it as a first argument. Plain
prelude functions beside the `->`/`doto` family (`std/prelude.blsp`); where `doto`/`->` splice
*forms*, these take a *function value*. The roadmap had them marginal (`->`/`doto` cover the
common cases); shipped for the exact Elixir spelling. Tests in `tests/ergonomics_test.blsp`
(5 cases incl. pipeline composition and an `:isolated` cross-process case). `docs/language.md`
+ `ROADMAP.md` updated; no ADR (trivial additive prelude fns).

## 2026-08-13 — Stdlib namespacing, stage 1: the `enum` sequence-helper module (ADR-227)

Began the stdlib namespacing program: *core* stays bare in the prelude, *derived helpers* move
into namespace modules. Stage 1 extracts 14 boot-independent sequence helpers (`dedupe`,
`frequencies`, `group-by`, `chunk-by`, `chunk-every`, `interpose`, `interleave`, `scan`,
`zip-with`, `min-by`, `max-by`, `reduce-while`, `enumerate`, `index-where`) out of the flat
`std/prelude.blsp` into a new `std/enum.blsp` (`CORE_MODULES`), and adds the new `enum/distinct-by`
(keyed uniq). The core collection protocol (`map`/`filter`/`reduce`/`fold`/`take`/`drop`/`distinct`/
`take-while`/`partition`/`zip`) stays bare — several are load-bearing during the prelude's own
bootstrap (it *is* the boot image and `require` is defined near its end, so it cannot require a
module), which conveniently coincides with the ops that should stay bare anyway. Template: `path`.

Consumers migrated with `(:use enum)` (`stats`, `tool/debug`, `tool/observer`, `editor/formbuf`,
the `nest new` editor scaffold template, and six test files) or `(require 'enum)` + qualified calls
for header-less scripts (`breakage/chaos_map_volcano`, `breakage/chaos_type_blender`, a `divan`
bench). A `/code-review high` pass caught the four consumers outside `nest test`'s reach
(`examples/`, `breakage/`, the scaffold template, `benches/`) — the class a full-suite green run
misses — plus a stale `min-by`/`max-by` docstring ("errors on empty" → actually returns `nil`,
corrected). Removed the 14 now-stale bare entries from `std/doc-catalog.blsp` (module fns are
documented via their module, as `set`/`json` already are). Full in-language suite green (4643/4643);
`nest format --check` clean; a scaffolded editor project checks clean. ADR-227 records the
principle + the boot-image constraint + the staging; `docs/language.md` + `ROADMAP.md` updated.
Next stages: `map`-extras, `math`.

## 2026-08-14 — Stdlib namespacing, stage 2: the `map` transformation-helper module (ADR-227)

Second stage of the namespacing program (ADR-227). Moved the four derived map-transformation
helpers — `merge-with`, `update-vals`, `update-keys`, `select-keys` — out of the flat prelude into
a new `std/map.blsp` (`CORE_MODULES`). The core map protocol (`assoc`/`dissoc`/`get`/`keys`/`vals`/
`contains?`/`reduce-kv`/`update`/`get-in`/`update-in`/`merge`/`zipmap`) stays bare — `merge` and
`zipmap` are called by the collection protocol / record machinery during bootstrap, and the rest
are the universal map surface. The module is named `map`; the bare `map` *function* is unaffected
(a bare `map` resolves to the function — there is no `map/map` — and the module's own helpers reach
it the same way; verified by smoke test). Consumers migrated with `(:use map)`: `enum` (its
`group-by` builds the result with `map/update-vals`, the one cross-module dep, which is why enum
went first last commit), `editor/ui`, `editor/buffer`, `maps_test`. A repo-wide sweep (incl.
`examples/`, `breakage/`, `benches/`, scaffold templates, `crates/`) found no other consumers.
Removed the four now-stale bare entries from `std/doc-catalog.blsp`. Full in-language suite green
(4643/4643); `nest format --check` clean (359 files). `docs/language.md` (map table + module
table), ADR-227, `ROADMAP.md` updated. Next: `math`.

## 2026-08-14 — Stdlib namespacing, stage 3: the `math` library module (ADR-227)

Third stage of the namespacing program (ADR-227). Moved the derived math *library* — `sqrt`,
`pow`, `ceil`, `round`, `round-to`, `clamp`, `abs`, `sum`, `product`, the sign/parity predicates
(`positive?`/`negative?`/`even?`/`odd?`) and the constants `pi`/`e` — out of the flat prelude into
a new `std/math.blsp` (`CORE_MODULES`). Core arithmetic stays bare: the operators, `quot`/`mod`/
`rem`/`floor`/`min`/`max`, and `zero?`/`nan?`/`infinite?`. Two bootstrap dependencies were inlined
so the bare prelude needs nothing from the module (it *is* the boot image, cannot require one):
`mod`'s `(abs b)` and the `binding` macro's `(odd? …)`. ~26 consumers migrated with `(:use math)`
(and `:only [abs]` in `telemetry_metrics_test`, whose `sum` is `telemetry/sum` — a `:use`-level
clash with `math/sum`).

**The instructive bug.** The first migration pass scanned only *operator-position* calls
(`(even? x)`) and missed the far more common *higher-order* uses (`(filter even? xs)`,
`(chunk-by even? xs)`, `(sort-by abs xs)`) — a value-position reference needs the import just as
much. Five files (`prelude_enum`, `queue`, `record`, `spy`, `stream`) slipped through and were
caught only by the full suite (`even?` unbound). A comprehensive symbol-level + HOF scan then found
them all; `min-by`/`max-by`-style false positives (`abs`/`sum` as *bindings* or in *strings*/*test
descriptions*) were filtered out by eye. This — plus the `:use`-clash errors — is why the next step
replaces hand-written imports with **auto-derived** ones (an unresolved bare name that one curated
stdlib module exports auto-refers it, lowest priority). Full suite green (4643/4643); `nest check` +
`format --check` clean. ADR-227, `docs/language.md`, `ROADMAP.md`, doc-catalog updated.

## 2026-08-14 — Auto-derived imports go live + stdlib namespacing stage 4: `json` (ADR-227 follow-up)

The mechanism the earlier stages leaned toward now exists. A **qualified reference
`mod/name` infers `(require 'mod)`** — you never write a `require` line just to satisfy
a `mod/…` reference. New `crates/lisp/src/eval/derive.rs`, wired into the `compile` pass
(`macros.rs`) via three hooks, each firing only on a `/` in a symbol:

1. `require_qualified_head` — **eager**, from `macroexpand_1`: a qualified *macro* head
   must load before the macro lookup (macros expand at compile time), so its module is
   required immediately.
2. `record_qualified` — **deferred**, from `resolve_sym`: records the module of a resolved
   qualified reference (a value in argument position) on a thread-local; `drain_pending`
   requires them after resolve, before eval.
3. `scan_root_refs` — at the **root region** (a header-less script / the REPL, where
   `resolve` is identity): scans the form so a top-level qualified value auto-requires
   too. Gated so it never runs during prelude boot.

**The design flipped from stage 3's plan.** Stage 3 anticipated bare-name magic (an
unresolved bare `sqrt` auto-refers `math`). We didn't ship that: *there is no bare-name
magic.* A bare `sqrt` with neither a `math/` prefix nor `(:use math)` stays unbound. The
rule is one line — *name where something comes from, and it loads on demand.* `(:use mod)`
still refers a module's names bare and still needs no separate require.

**GC safety.** `compile` can now collect at any depth (an inferred require loads a module),
so every LOCAL handle held across it is rooted and re-read or it goes stale (use-after-GC):
`resolved` in `macros.rs`, and two re-reads of the relocated form in `check.rs`'s
`check_file_ext` (the error path and the header-check path).

**Checker.** The KI-17 *"reference to an unrequired module"* lint (`unrequired_module`,
`walk.rs`) is now **permanently obsolete** — a qualified reference can no longer reference
an *unrequired* module, because the reference itself requires it. Neutralized to a no-op;
its reachability scaffolding (`required_mods`/`raw_qualified`) is retained (still touched)
for possible repurposing.

**Stage 4 (`json`).** With the mechanism in place, `std/json.blsp`'s exports drop their
now-redundant `json-` prefix — `json-parse`→`parse`, `json-encode`→`encode` (referenced
qualified as `json/decode`, the prefix was doubled). Consumers updated with no new `require`
lines needed anywhere: `std/net/sse.blsp`, `std/tool/{docs,explain,grammar,package,test}.blsp`,
`tests/*_test.blsp`, and the JSON fuzz target. The stage-1/2/3 test consumers move to the
qualified spellings too (`math/even?`, `enum/frequencies`, …) in `basic.rs` and the
checker's `soundness_oracle`/`tests` fixtures.

Verified: full in-language suite green (single-process run, 470s, exit 0); the std-wide
zero-warnings gate `nest check std/**/*.blsp tests/**/*.blsp` clean; workspace build +
Rust tests green. See `docs/auto-derived-imports.md`; ADR-227.

## 2026-08-14 — The computed-head call pays no allocation: the arm handle is memoized (ADR-228)

Perf session. Recovered the `pipeline` regression that the 2026-08-14 benchmark round had
bisected to ADR-224 and priced as unavoidable, and took `nqueens` with it.

**The tree was not green when this started — worth recording because of *where* the red was.**
`make test` failed 6 cases at `26b04e36`: four in `crates/lisp/tests/basic.rs`, two in the
checker's `types/check/soundness_oracle.rs`, all unbound-symbol errors for names ADR-227 had
just moved (`even?`/`positive?`/`sum`/`abs` → `math/`, `frequencies` → `enum/`). Same class as
ADR-227's own migration lesson (a reference in *value* position), with the twist that these live
in **Rust** files that `eval_str` bare source — so the in-language suite reported green
(4643/4643) while six Rust-side cases were red, because that suite structurally cannot see
them. The lasting point: after a stdlib move, the migration scan has to cover
`crates/**/*.rs` string literals, not just `.blsp`. **The fix that landed is `a57cc573`'s**
(qualified names, no `require` — auto-derivation resolves them); the duplicate written here
before that commit arrived was dropped in the merge. The rest of the gates were clean at that
tree, both engines: `make test-both` 981 + 981, breakage 23/23,
`nest check`/`format --check`/clippy.

**The change (ADR-228).** `exec_chunk`'s call arm says a **computed head takes no inline
cache** — so `dispatch`, `exec_chunk` and the JIT's non-elided resolve all reached
`compiled_arm_for` *per call* and wrapped it in a fresh `ArmHandle`: one `Arc::new` **and** one
clone of the shared `Arc<CompiledArm>` (an atomic RMW on KI-40's cross-process cache line) for
every transducer step, callback and message handler. `compiled_arm_for` now returns the handle,
memoized per `(closure, argc)` in the `vm_cache` entry it already consults.

**Measured twice, against two bases, and the runs disagree — so the ADR records a range, not the
flattering figure.** `pipeline` −9.1% vs `26b04e36` and **−5.6% vs `a57cc573`** (both best-of-15,
default ceiling); at ceiling 1, −6.2% then −4.7%. `nqueens` at ceiling 1: −6.2% then −4.4%.
`primes`: −4.3% then −5.7%. `pipeline` improves on every run and at both ceilings, so the
direction is settled and ADR-224's +9.3% is substantially recovered, but the size is uncertain
within ~5–9%. `nqueens` and `primes` each straddle the `max(5%, 2×floor)` gate depending on the
run, so neither is claimed as certified. `pfib` ceiling-1 −1.9%, so ADR-224's 3.19× is untouched.
The lesson to carry: two best-of-15 runs of the *same* comparison differed by ~3.5 points, and
the `floor` column read 0.0% on several rows — at integer-millisecond output that means "below
the resolution", not "no noise". Pinning a few-percent row wants a fixed baseline binary plus a
base-vs-base control, which was not run here.

**Two bugs the review caught, one race behind both.** "Put, then read the handle back out of
the cache" is unsound as written: the read starts with `sync_free_epoch`, and a *peer* process
can advance `free_epoch` at any instant (no stop-the-world), clearing the entry just inserted —
so `compiled_arm_for` could report "no VM arm" for a closure that compiled fine, silently
tree-walking a compiled body. The same race made `vm_run_bc`'s `is_some()` guard plus
`.expect("just checked is_some")` a live **panic** vector on a process body. Both fixed (fall
back to the arm in hand; resolve once).

**`perf` works on this box again** (`kernel.perf_event_paranoid` 4 → 1), so the frontier's
attribution is measured rather than inferred for the first time since 2026-07-03. `pipeline` at
N=10M, self: `dispatch` 15.1%, `jit_dispatch_call` 8.1%, `push_frame` 6.3%, `passthrough_arm`
5.3%, `Heap::closure` 5.0%, `vm_cache_arm_handle` 4.8%, `compiled_arm_for` 3.1%, `select_arm`
2.0%, `vm_arm_block` 1.2%, plus ~8% of SmallVec argument staging — i.e. **~50% of the row is
call plumbing**, and the pieces are individually fixable. Note `release-fast` sets `strip = true`,
so a profiling build wants `CARGO_PROFILE_RELEASE_FAST_STRIP=false
CARGO_PROFILE_RELEASE_FAST_DEBUG=line-tables-only make release-brood` (same codegen, symbols kept).

**Next lever, now quantified rather than guessed:** give computed heads a one-way monomorphic
inline cache keyed on closure identity, holding `(passthrough?, handle, cenv, bases)` together —
`exec_chunk` already does exactly that identity check for *staged* heads. That collapses
`passthrough_arm` + `select_arm` + `vm_arm_block` + the memo lookup + much of `Heap::closure`
into one guarded slot read (~18% of the row), and reaches every callback/handler workload.

**Two test-harness races fixed on the way through, because "known flake" is not a status
(KI-43).** The merged tree's `make test` came back red twice over, and neither failure was in
the change:

- **`remote_attach_reads_snapshot_then_sees_disconnect` killed the target after a fixed 5 s
  sleep.** It failed *both* retries under load and passed standalone — the signature that
  normally gets written off. Measured under saturating load (14 busy loops on 12 cores), the
  case needs **5.9–9.2 s**: every sample above the deadline, so under load it was arithmetic,
  not luck. Now waits for the observer's own attach report; **8/8 under that same load**, and
  3.5 s instead of ~11 s idle because the unconditional sleep is gone. Two earlier sessions had
  already bumped that constant (1500 → 5000 ms) — the same fix applied twice to the same wrong
  idea. Confirmed in the live gate afterwards: it PASSed at **11.5 s**, i.e. the old code would
  have failed that run too.
- **`basic.rs`'s `named_spawn_respawns_after_death` had the identical shape, undetected** — a
  fixed 50 ms sleep standing in for "the scheduler ran and deregister fired", then a positive
  assertion. Polls now, 10 s backstop.
- **`completion_never_fails_however_it_is_called` timed out at the 120 s default.** Not a
  regression: 96 child-process spawns, **60.7 s solo at `a57cc573` vs 59.0 s here**, and a
  previous session had already trimmed the matrix once. Given its own 300 s budget with the
  measurements recorded; it then PASSed the gate at **147.8 s**, so the old cap would have
  killed it again.

Audited every other fixed sleep in the Rust harnesses: the rest are retry cadences, physical
waits (`stale.rs`'s 1100 ms for mtime granularity), or gates on *negative* assertions where a
short sleep costs sensitivity rather than correctness. The 59 `(sleep …)` sites in the
in-language suite are covered by repetition rather than a blind rewrite — the campaign this
session ran the real-TCP/boot-wait family 10× under load, the whole suite 6×, and the JIT tests
3× under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`.

**Also found, not fixed (follow-up).** `gc_runtime.rs`'s compaction flush (step 3b, ~line 1224)
walks `live_vm_arms` *without* the distinct-arm dedupe its sibling probe walk (~line 695) carries
— and that walk's own comment explains why the dedupe exists: the registry is a per-frame stack,
so a 100 000-deep recursion holds 100 000 entries that are all the same `Arc`. Not a correctness
bug (`flush_rt_value` only forwards handles in the source generation, so repeats are no-ops), but
a deep process being compacted pays O(frames × arm size) for O(distinct arms) of work — the
KI-14 shape, in the walk that never got the fix. Same `seen_arms` set applies.

## 2026-08-14 — Remove the user-facing `require` form; loading is inference + an internal `require-one`

Follow-through on the auto-require work: **there is no longer a user-facing `require`.** Normal
code loads modules by *referencing* them — a qualified `mod/name` auto-infers the load, and
`(:use mod)` refers + loads — so the explicit `(require 'mod)` line is gone from every
hand-written `.blsp` file in `std/`, the tests, and every `../*` sibling project (~450 call sites).
`(defn require (& mods) (map require-one mods))` was deleted from the prelude.

**A loader still exists, deliberately in Brood.** `require-one` (load-path search + cycle detection
+ feature tracking) stays as the internal loader — it can't be a Rust `%`-primitive without moving
policy out of Brood (violating ADR-006). It is called only by the irreducible cases: the
`:use`/`:alias`/`:use-internals` macro expansion, the genuine **project↔package mutual cycle**
(where inference would load a half-built module mid-cycle), dynamic loads by a *computed* name
(`docs`/`project` folding over module lists), and a handful of pure effect-loads in the Rust
bootstrap (`(require-one 'test)` — load the framework where no `test/` reference follows). Where a
qualified reference already covers the load, the explicit call was **dropped**, not renamed: of the
Rust `eval_str` bootstrap sites, **70 were dropped** (inference covers) and **24 kept** (effect-loads
/ format-placeholder refs).

**What the removal touched beyond the call sites:**
- The `:use`/`:alias` expansion now emits `(require-one …)`, not `(require …)`.
- The **advisory checker** hardcoded the symbol `require` in three places (`is_require_form`,
  `require_target`, and `ensure_loaded` — which *builds and evals* a load form so it can read a
  `:use`d module's exports for the unused-import lint) plus the effectful-forms list in
  `guard_effects`. All now recognise `require-one`. (Missed first pass → `unused_use_import_is_flagged`
  regressed because `ensure_loaded` evaluated an unbound `require`.)
- **Auto-require completeness fix:** the root-region scanner `scan_refs` (`eval/derive.rs`) recursed
  into pairs and vectors but **not map/set literals**, so a qualified reference used only inside a
  `{…}`/`#{…}` at the top level of a header-less script never inferred its load. It now recurses into
  maps and sets. (Surfaced by `serve_attach`'s client, whose sole `editor/serve/serve-name` reference
  lives in a map literal — the dropped `require` had been masking the gap.)
- Rust/LSP/error-hint strings that suggested `(require 'mod)` (reader regex hint, mailbox reconnect
  hint, `debug_flags`/`builtin-modules` help, `derive.rs` module doc) were reworded off `require`.
  The LSP `require`-argument features (document-link / completion / definition / the "Add require"
  code action) are now dead code — flagged for removal, non-breaking.

Verified: **full `cargo nextest` run green — 981 tests pass** (the in-language suite at 439–450s
plus every Rust test), no crash, capped at 4 threads. The sibling projects (`hive`, `hatch`,
`pong`, `bedit`, `store-postgres`, …) were migrated in parallel and each `nest check`s clean.

## 2026-08-17 — The stale-bare-name sweep: a curated sig masking the unbound lint, two dead benchmark rows, and one GC walk

Follow-up session to ADR-228. Four queued items; this covers two of them plus two findings
that were not on the list.

**The checker's type tables had six stale entries, and one was a hole in the CI gate.** Swept
mechanically rather than by eye — extracted all 101 keys from `sigs.rs`, `infer.rs` and
`walk.rs` and tested each against `bound?` at root. An entry in `CURATED_SIGS` marks its name
*known*, which **suppresses the unbound lint**, so a stale key makes `nest check` — the gate
that exits nonzero on any warning — silent on code that dies at runtime. Proven by contrast:
after ADR-227 moved `even?`/`odd?`/`abs` to `std/math.blsp`, a bare `(even? 4)` with no import
drew *nothing*, while the uncurated siblings `sum`/`frequencies` correctly said "unbound
symbol". Worse, **`length` had never existed at all** — added 2026-05-31 as if it were a `count`
alias ("each vetted against std/prelude.blsp"; this one was not), so `(length x)` had been
passing `nest check` for two and a half months. Fixes: `length` deleted; `even?`/`odd?`/`abs`
re-keyed `math/…`, `index-where` `enum/…`; `infer.rs`'s `sqrt`/`abs`/`dedupe`/`interpose` and
`walk.rs`'s `abs` re-keyed too (those are merely *dead* — they supply a type, they do not mark
known-ness). Guard `a_curated_sig_does_not_mask_the_unbound_lint_for_a_moved_name`,
sabotage-verified (restoring a bare key fails it with `Got: []`). Five existing tests asserted
the old spellings; one of them, `(length :k)`, had been passing *accidentally* — "some warning
mentioning length" is true of an unbound warning too, so it proved nothing.

**Two published benchmark rows were dead for three days (KI-44).** `nbody` died with `unbound
symbol: sqrt` and `json` with the dropped `json-` prefix: ADR-227's migration sweep covered
`breakage/`, `examples/`, `stress/`, `std/` and `crates/` but could not see `brood-benchmarks`,
a separate repo. A published harness run would have failed outright. Both fixed by qualifying the reference (which loads the module by
inference) and verified against the other ports' checksums, not merely "it runs now".
(Those two figures — `nbody` −169063618, `json` 364568836 — turned out to be **stale**;
see the warning in KI-44. `nbody`'s checksum is N-dependent, so quoting one without its
`BENCH_N` is meaningless. `results/results.json` is canonical.) The structural cause — that
nothing runs those programs for *correctness*, only for timing, by hand, over tens of minutes —
is fixed by `bench/smoke.py`: every row at the harness's own quick sizes, exit status only,
about a minute, sabotage-verified — and it immediately paid for itself: ADR-229's `require`
removal landed while this was in flight and broke `base64`, `json` and `regex` in that repo,
which the check caught in one run. (First attempt used a flat `BENCH_N=50` and reported three
false failures, because `BENCH_N` is an iteration count on some rows and a *problem size* on
others — 50 means fib(50) on `pfib` and a 50×50 board on `nqueens`. It now imports the
harness's `QUICK` table so the two cannot drift.)

**Fixing `nbody` correctly then exposed a ~1.8× regression, left open deliberately.**
`resolve_prim1` inlines `sqrt` to `PrimOp1::Sqrt` only for a **bare** head resolving to a
**PRELUDE** closure, and post-move neither spelling qualifies — so every `sqrt` now pays a
closure call plus the wrapper's two `cond` comparisons. Measured: 406→754 ms on a 3M-iteration
loop (~115 ns/call) and **0.38–0.40 s → 0.66–0.74 s on the row**. The remaining work is the
*safety* argument, not the name test: the PRELUDE-region check has to keep proving "this is the
canonical `std/math` sqrt, not a redefinition" for a closure that now lives in RUNTIME. Recorded
in KI-44 and flagged at the top of the benchmark repo's `nbody` entry, because **an `nbody`
number measured before it lands is ~1.8× off its pre-ADR-227 self and is not a runtime
regression.** The generalisation: a kernel fast path keyed on a bare stdlib name is a hidden
coupling to the stdlib's shape — moving the function is source-compatible and silently deletes
the optimisation. When a stdlib function moves, grep the kernel for its bare name.

**GC: the compaction flush now visits each distinct arm once.** `live_vm_arms` is a per-frame
stack, so a deep recursion holds one entry per active frame, all the same `Arc` — the flush
re-rewrote one arm once per frame, O(frames × arm size) for O(distinct arms) of work, the KI-14
shape in the sibling walk that never got the dedupe its twin carries. Never wrong
(`flush_rt_value` only forwards source-generation handles, so repeats are no-ops), which is
exactly why it hid.

Gates: `make test` 983/983; `make test-both` 983+983 with **zero flaky**; `runtime_collector`
and `jit` each under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`; breakage 23/23; `nest check`; `nest
format --check`; clippy `-D warnings`; fmt. A methodology note for the next session: two
overlapping `make test-both` runs (my error) produced three fixed-deadline failures that a
clean run did not reproduce — see the KI-43 addendum. Don't run two suites on one box and read
the result as signal.

## 2026-08-17 — `nest release` bundle rooting: dev/release parity for package-rooted deps (ADR-070)

Closed the last ADR-070 follow-up. A release bundle used to embed every module by its bare
require-name, so two dependencies each providing a `parser` collided and `bundle-collect`
rejected the build (`bundle-reject-duplicate-names`) — the interim SAFE stop. Now a dependency's
modules are embedded under their **rooted** key (`foo/parser`), so two same-named dep modules
coexist in one flat bundle, exactly as they stay distinct at dev time. Brood-only change (no
Rust): the archive already keys modules by an arbitrary string and `%builtin-module` already
serves bundled deps.

Three edits, mirroring the disk package-context path (`require-force-package`):
- **`bundle-collect`** (`std/tool/project.blsp`) keys dep modules from the `*package-module-files*`
  map `ensure-deps` already builds (rooted-name → path), filtered to files under a bundled dep
  dir so it is independent of any earlier `project-setup`. Root-project modules stay by their bare
  require-name (a single package can't collide with itself; the entry still resolves `main/main`).
- **`run-bundle`** rebuilds `*package-modules-of*` at boot from the embedded manifest's dep names
  + the embedded keys (`bundle-register-dep-rooting`), before loading any module.
- **`require-force`'s embedded-load branch** (`std/prelude.blsp`) now sets a bundled dep's package
  context via a new `bundle-module-package` helper instead of always clearing it. It returns nil
  for std and for the bundle's unrooted root-project modules (both load at ROOT). In a non-bundle
  dev run the branch only ever fires for std — whose namespaces are never packages — so the check
  is a no-op there; a dep loads from disk (`%builtin-module` returns nil).

Full root-project bundle-rooting (the Elixir-uniform `myproj/main/main`) has no consumer and
stays deferred (ADR-011); the reject-guard is kept as a loud backstop for a genuine key clash.

Coverage: `crates/cli/tests/release_bundle.rs` gains `bundled_deps_with_same_module_name_coexist_rooted`
(end-to-end — two deps' `parser` each root their own `(:use util)`, printing `alpha:A|beta:B`);
`tests/package_test.blsp` gains a bundle-collect case asserting deps key rooted and the root
module stays bare. Baselines re-run green: release_bundle 3/3, namespace 51/51, package 110/110,
every in-language `tests/*_test.blsp` file with no failures.

## 2026-08-17 — Close the last two open issues: the `sqrt` call-site inline (KI-44) and the stale `examples/editor` (KI-45)

Two open items from the day's earlier sessions, both now green.

**KI-44 — the `sqrt` call-site inline, restored via a structural identity.** ADR-227 moved `sqrt`
out of the prelude into `std/math.blsp`, which killed the `resolve_prim1` inline: it required a
*bare* `sqrt` head resolving to a sealed PRELUDE closure, and the head is now the qualified
`math/sqrt` bound to a RUNTIME closure. `region == RUNTIME` alone cannot identify the canonical
wrapper — a user's own `foo/sqrt` is a RUNTIME closure too — so the inline is now keyed on a
STRUCTURAL match: any `…/sqrt` (or bare `sqrt`) head bound to a closure whose single 1-param arm is
exactly `(if (< n 0) _ (if (<= n 0) _ (%f64-sqrt n)))`, with `<`/`<=` the canonical PRELUDE
comparisons and `%f64-sqrt` the native. That shape is what makes the `PrimOp1::Sqrt` x>0 shortcut
sound (a positive argument provably returns `%f64-sqrt(n)`; everything else deopts to the live
wrapper via the stored head). Rebind-safe for free — `Inst::Prim1` re-runs `resolve_prim1` on every
`global_epoch` change — and degrades to no-inline on any deviation, never a miscompile. Discovered
along the way that `math/sqrt` is a *reserved* name (`(def math/sqrt …)` → E0030), so the global
binding can't be hot-reload-rebound at all; the structural check still matters for user `…/sqrt`
names, which the `/sqrt` name-gate also reaches. Measured (release, pinned, 3M-iter loop): inline
**321 ms** vs wrapper-dispatch **905 ms**. Guarded by the whitebox unit test
`sqrt_call_site_inline_recognizes_the_moved_math_wrapper` (canonical wrapper inlines; bare `sqrt`
unbound; a user `usersq/sqrt` computing `n*n` does not inline).

**KI-45 — deleted `examples/editor`.** It had referenced `eval-command/eval-last-sexp` since that
module moved to the sibling `brood-edit` project on 2026-05-31, so 1 of its 5 tests failed. Chose
option (a): brood-edit is the real editor project, so the in-repo duplicate was removed rather than
kept limping (also removes 9 of the ADR-229 migration's edits). `scripts/check-examples.sh` no
longer skips a known-red project (`SKIP_PROJECTS` is now empty), and the `docs/layers.md` /
`builtins/system.rs` pointers to `examples/editor/src/` were repointed at brood-edit.

`docs/known-issues.md` now shows **no open items** — only the KI-36 watch item (unreproducible on
demand) remains.

## 2026-08-17 (later) — the computed-head resolution memo: implemented, measured, reverted

The frontier's lever 3, sized at ~18% of `pipeline` from that morning's profile: a **computed
head takes no inline cache**, so `passthrough_arm` (5.3%), `select_arm` (2.0%), `vm_arm_block`
(1.2%) and the arm-handle lookup (4.8%) are re-derived on every call — every transducer step,
callback, message handler and `(f x)` where `f` is a parameter — and none of them can change
between two calls to the same closure at the same arity.

Built it the low-risk way: the memo rides in the existing `vm_cache` entry, so there is **no new
table** (the per-process floor is the runtime frontier's lever 1) and **no new invalidation
obligation** — verified first that every `arm_ic_blocks` clear is paired with a `vm_cache` clear
in the same function, which is what makes caching `bases` sound, and that the one unpaired
direction (`sync_free_epoch` clears only `vm_cache`) is the safe one. All five call sites
rewired, including the JIT's non-elided resolve. Suite 4650/4650, 477 lib tests.

**Reverted on measurement.** Against a pinned baseline (`make ab-pin`, added for exactly this):
`nqueens` ceiling 1 **−4.3%/−4.9%**, `pipeline` ceiling 1 −1.6%, `sort` −1.3% — but `pipeline`
at the **default** ceiling was **parity** interleaved at N=10M (3410/3420 base vs 3430/3420),
and `spawn-live` peak RSS went **647→693 and 651→699 MB, +7.0%** (≈+135 MB at the published
300k units), because the memo adds ~24 bytes per `(closure, argc)` per process. No gain where
users run, ~5% on one row at ceiling 1, and a 7% memory regression on the row whose open work
item is *reducing* the per-process floor. Bad trade; the patch stays out of tree.

**Two findings worth more than the change would have been.**

1. **The profile over-promised, because the derivations are cheaper than the memo that replaces
   them.** A closure deref plus a `max_by_key` over a single-arm closure costs less than a
   `HashMap` probe, so converting 13% of profiled self-time into one memo lookup bought ~0. A
   share in a profile is an upper bound on what removing that code *could* pay, not a share you
   can collect — the replacement has to be cheaper than what it replaces, and here it was not.
2. **An intermediate version cost `reduce` +5.0%** because it resolved an arm even for thin
   wrappers — i.e. it *compiled* every `+` just to memoize it — and that row is almost entirely
   passthrough calls. Caught by measuring rather than by reading, which is the argument for
   measuring each step rather than only the finished change.

Also this session: `make ab-pin` / `scripts/ab-pin.sh` — a **pinned** baseline binary
(`target/ab-pinned/<sha>/`, outside `target/ab/` so `make ab-clean` leaves it) plus a
base-vs-base floor. `make ab` rebuilds its baseline in a throwaway worktree, so two runs on
different days measure against two different binaries; ADR-228 hit exactly that and had to
record a range (−9.1% and −5.6% for the same comparison) instead of a number. The reverted
change above was measured with it, and one interleaved 10M-element run settled a question two
`ab-bench` sweeps had left ambiguous.

What the negative result implies for the frontier: the computed-head path's cost is the **call
protocol itself** — frame setup and dispatch — not the resolution bookkeeping. That points back
at call inlining and the interpreter's dispatch loop, and lever 3 is demoted accordingly.

## 2026-08-18 — KI-39 was never a flake: a coloured log, an uncovered TIMEOUT, and a keypress that loaded 2967 lines

Three CI sightings of the `differential (tree-walker)` job had failed to name a single case.
With `gh auth` restored, the artifact was finally readable and the whole thing unwound in one
sitting. Two independent bugs, one hiding the other.

**The instrument was blind to colour.** nextest colours its output under CI even when piped to
a file, so the log holds `Summary<ESC>[0m [1922.084s] …`. Every pattern in the annotate step
anchors on `WORD [`, so `Summary \[` and `FAIL \[` matched **zero lines** of the real log —
verified by replaying it. Local logs are uncoloured, which is exactly why the greps looked
right here and were dead there. Worse, the failure was a **`TIMEOUT`**, a line shape no pattern
covered at all. Fixed at the source (`--color never`), in the step (grep an ANSI-stripped
copy), and in coverage (`TIMEOUT|SIGSEGV|SIGABRT|ABORT|LEAK` alongside `FAIL`). Replaying the
real log through the patched step now annotates the case in one line.

**The failure: `nest complete` loaded the whole project on every TAB press.**
`completion_never_fails_however_it_is_called` spawns 96 `nest complete` subprocesses. Each one
that reaches a value position boots an interpreter and requires `complete` — which opened with
`(:use-internals project)` for four twenty-line helpers, and so loaded all 2967 lines of
`project`, plus `scaffold`, plus `project` again behind it. Debug build, measured: **950 ms per
completion, 770 ms of it that module load**; 96 × 950 ms = 64 s *alone on an idle 16-core box*,
against nextest's 60 s slow line. The module's own header carries the rule it was breaking —
"NEVER load the project image" — from the very first line of its `defmodule`.

`std/tool/complete.blsp` is now dependency-free: a local root walk, one generic
`complete-under` file collector, the manifest read as data, and a two-line `complete-plist-get`.
Templates keep their `scaffold` dependency but pay it at call time (`require-one` inside the
function) instead of by naming an export at load time, which under ADR-229 auto-requires. The
module load went **848 ms → 24 ms**, a completion 950 ms → 121 ms (debug) and 92–123 ms →
**18–19 ms** (release), and the test **63.8 s → 10.5 s**. All ten completion kinds were diffed
byte-for-byte against the pre-change binary before and after.

Method note, because the two release figures were not taken the same way: the pre-change one is
single runs of the 2026-08-17 binary (measured before that binary was overwritten), the
post-change one is best-of-5 on a warm boot cache. The gap is 5–6×, far outside anything
best-of-N accounts for — but the debug pair is the rigorous one (same method, same fixture, both
sides), and the release pair should be read as indicative.

**It was never intermittent.** The entry called it a 3-of-11 flake; with the log readable,
all 8 runs that day failed it, in *both* jobs that run the suite, on the same case. What varied
was only whether the box was slow enough to cross the 300 s cap. A fixed cost against a fixed
deadline looks random and isn't.

**The latent bug the rewrite exposed.** The old code read `*project-source-paths*` /
`*project-test-paths*` — ambients only `project`'s manifest loader sets, which completion never
ran. So a project declaring `:source-paths ["lib"] :test-paths ["spec"]` completed **nothing**:
no test files, no tags, no modules. Reading the manifest as data fixes it, verified on a
fixture (`spec/w_test.blsp`, tag `integration` and module `widget` now offered; all three were
empty before).

**And the gate hole underneath all of it.** `nest check` checks `:source-paths`, which in this
repo is `tests/support` — the standard library is embedded, not built as a project, so **nothing
was checking the ~80 files every Brood program loads**. That is what let the rewrite keep a call
to `project`'s `plist-get`: the reference was unbound, but every path in that module is wrapped
in a `complete-safely` net by design, so `nest remove <TAB>` just silently offered nothing and
no test failed. A module whose contract is "never raise" cannot be guarded by watching for
raises. New suite gate `tests/std_check_test.blsp` runs `check-file` over every `std/**/*.blsp`
(7.3 s; `check-file` resolves each file's own requires, so it is order-independent) and asserts
zero warnings — sabotage-verified in both directions, and it names `file:line: unbound symbol:
plist-get` when the bug is reintroduced. std/ is otherwise clean at 80 files, and the one
warning the rewrite legitimately introduced (an ambient bound by a runtime `require-one`) is
suppressed with `(check-allow :unbound …)`, the documented case for that lint.

## 2026-08-18 (later) — the margin audit's own tail: KI-46, and a pre-push hook that only watched half the formatters

CI came back **5 of 5 green** on `615b06be` — the first fully green run since 2026-08-14, and
confirmation that KI-39 is dead. The green run's own numbers, from its tree-walker job:
`completion_never_fails_however_it_is_called` **22.0 s** (it had been a 300 s TIMEOUT),
`template_default::ships_passing_tests` **9.6 s** (was 89 s as a looping case), the whole suite
**1217 s** against 1922 s, and **1 slow case** against 9.

Two follow-ups closed with the tree green.

**KI-46, the real fix rather than the documented plan.** The audit had left
`std_check_tool_returns_structured_diagnostics_or_an_error` at 87 s (1.38× margin) with a note
that the honest fix was an optional root argument. Done: `check-project-structured` takes an
optional `from` (defaulting to `(cwd)`), and `mcp-check-tool` passes **`*project-root*`**. This is
a consistency fix, not a test accommodation — the server serves one project, `*project-root*` is
what its write sandbox is pinned to and what `project-all-files` already reads, and `check` was
the single tool taking its project from wherever the host process was standing. **87 s → 2.5 s.**

The interesting part was that the first version of the rewritten test was *too weak*, and the
sabotage pass caught it: with `check` reverted to the cwd fallback the test still **passed**, just
slowly (2.9 s → 21.7 s) — the same "a cost regression shows up only as slowness" failure this
whole thread is about. So the fixture now plants a deliberate unbound-symbol warning and the test
demands it back with `:file`/`:line`, plus asserts that no diagnostic comes from outside the temp
root. Both sabotages (a stubbed tool, and the cwd fallback) now fail it. It also asserts strictly
more than the version it replaced, which accepted `{:diagnostics []}` *or* `{:error …}` and so
never proved the diagnostics path emits anything.

**The pre-push hook watched only one of the two formatters.** A local `pre-push` hook has guarded
`cargo fmt` since 2026-07-26 — but `nest format` is a *separate* gate, and it is the one that went
red in a push earlier this month (its exit code was ignored in favour of its last printed line).
The hook now checks both, and `make hooks` installs it from `scripts/git-hooks/pre-push` so it is
version-controlled instead of living only on one machine. `nest format --check` is whole-project by
design (`--changed` is ignored with `--check`, ~50 s), so it runs only when a `.blsp` is actually
involved — a Rust-only push stays at **0.8 s**.

Sabotage-verified, and the first attempt was wrong in an instructive way: gating on
`git diff $upstream..HEAD` alone meant a *dirty* `.blsp` was never checked, while the Rust half
checks the working tree unconditionally. The condition now looks at the pushed commits, the working
tree, the index and untracked files.

## 2026-08-18 (perf) — the cold-call tax measured: a refuted premise, a 3× attribution error, and a lazy FastLink mirror

With the tree green, back to the question this session opened with: the most profitable large perf
change. `FRONTIER.md` named its own candidate — lever 2's generalisation, "~0.85 µs for one call in
a cold process… whatever makes a first-call-in-a-process cheap would pay out across `spawn-live`…
that is unmeasured and is the thing to look at first". So it got measured.

**The tax is real; the premise about it was wrong.** The forwarder ladder reproduces at HEAD, 18%
faster than August with the slope intact: **19.45 / 21.05 / 21.90 / 24.45 CPU µs per unit** for a
direct `%vector-reduce`, one forwarder, two, and `fold`. But it is *not* a first-call effect —
calling the **same arm again in the same process** costs the same as the first call (+2.21 / +2.15
/ +1.40 µs for one/two/three nested `id1` calls), against a base-vs-base control of 1.5% on min
and 3.75% on median. There is no warm-up to remove. Compilation is not the cost either:
`BROOD_TRACE_COMPILE` counts a constant ~142 compiles whether the run spawns 100 processes or 400,
so ADR-215's sharing holds. A symbolized profile put the added cost in memory traffic —
`env_get` +560 ns/unit, kernel page faults +274, `is_dynamic` +232, `code_gen_pinned` +170,
`alloc` +164, `RwLock::read_contended` +128 — which is what redirected this at per-process *bytes*.

**Which found a 3× attribution error.** Instrumenting teardown on the real row: every unit process
allocates **14 call-IC sites**, of which only **6–7 are ever populated**, costing **1568 B**
(14 × [64 B `CallIcEntry` + 48 B `FastLink`]) — ~26% of the row's ~6.3 KB per-process footprint.
`FRONTIER.md` had IC tables at "~536 B". At 1568 B they are the *largest* single attributed item,
larger than the whole `Box<Process>` with its inline `Heap` (1376 B). `vm_arm_block` allocates an
arm's block whole on first entry, sized by the arm's total `nsites`, whether or not this process
will execute those sites — and a spawn-once-call-once unit never will.

**Shipped: the mirror is allocated by its only writer.** `vm_arm_block` no longer pre-grows
`vm_fast_links`; `Heap::fastlink_slot_grown` grows it on the first publish. Safe by construction,
not by luck — every reader already tolerated a short table (VM probe `.get`, both publish paths
`.get_mut`, and JIT'd code bounds-checks `site < len` against a length it re-fetches after each
Brood→Brood call *precisely because a cold nested call may grow and realloc this table*). A missing
slot reads exactly like an unpublished one.

Measured: **19,968 of 20,001 unit processes now allocate 0 slots instead of 14** (mean 1 B/process,
from 672). `spawn-live` RSS 6364 → 6093 B/process (−4.3%); 5821 → 5605 (−3.7%) with
`MIMALLOC_PURGE_DELAY=0`. Time-neutral at **both** ceilings — default: fib +0.0%, pfib −1.3%,
nqueens/pipeline +0.0%, sort −1.0%, spawn-live +0.6% against a 1.2% floor; tier 1 (`--tier 1`, the
gate a call-path change actually needs): all noise. `fib` is the load-bearing row, since the in-IR
fast-link is worth ~20% there — +0.0% is what proves linking still happens. 992/992 on both
engines. The first sweep's `spawn` +3.4% was drift: best-of-15 gives +1.7% against a 1.7% floor.

**The caution this leaves behind matters more than the win.** 672 B/process of allocation provably
removed bought 216–271 B/process of RSS — about **1:0.35** — and page granularity does not explain
it, because the saving stayed ~216 B with purging forced. So the two remaining IC fixes (shrink
`CallIcEntry` 64 → ~48 B; share entries for frozen callees) should be sized at roughly a third of
their allocated bytes until someone explains where the rest goes. On that arithmetic the shrink is
~80–120 B/process observed, which is marginal for its complexity.

### Correction, same day — the "1:0.35 allocation-to-RSS discount" was my own measurement error

The entry above closed with a caution that 672 B/process of removed allocation bought only
216–271 B of RSS, called the ratio ~1:0.35, and told the next person to size the remaining IC
fixes at a third of face value. **That is wrong. Retracted.**

The error: I compared a *measured* RSS delta against an *inferred* allocation delta. The 672 B came
from a slot count taken at process **teardown** (14 sites × 48 B). Measuring allocated bytes
directly with `(mem-bytes)` — which the runtime has exposed all along, via the `Counting` global
allocator — gives the real figure: live bytes after spawn 504,375,386 → 485,113,258, i.e. **192.6 B
per process**. Against that, the 216–271 B of RSS tracked the allocation *fully*. There is no
discount and there was never a mystery.

Why 193 and not 672: a unit parked in `receive` has entered only ~4 call sites' worth of arms. It
reaches 14 only after running its body, by which time most units have died and freed. Peak memory
is set by the parked state, because that is the state all N processes are in at once. Confirmed by
construction: adding 24 call sites to the unit body moved the measured saving to **1345.6 B ≈
193 + 24 × 48**, so the saving is exactly 48 B per call site entered.

Two lessons worth keeping, both of which cost this session real time:

1. **`(mem-bytes)` / `(mem-peak)` are the instrument for an allocation question, not RSS.** RSS
   answers "what did the OS map", which is a different question and a noisier one.
2. **A slot count is not a byte count until you say *when*.** The same structure measured at
   teardown and while parked differs by 3.5× here, and only one of the two is what peak memory
   sees. The `spawn-live` shape — spawn all, then release — makes the parked state the one that
   matters; a workload that ran each process to completion serially would show the other.

The change itself stands unaltered — same code, same time-neutrality, same 992/992 on both
engines. Only the size claim and the bogus caution change.


## 2026-08-18 — Stdlib namespacing, stage 5: the `string` library module (ADR-230)

Moved the whole string surface into a real `std/string.blsp` `(defmodule string)`, one name per op,
no bare forwarders (greenfield). The boundary rule: **`string/*` = "the first argument is a
string"** (trim/pad/case/search/split-join/substring/Unicode + the char-content conversions
`to-list`/`from-list`/`to-codepoints`/`from-codepoints`/`to-graphemes`); **bare** = the polymorphic
collection ops (`index-of`/`includes?`/`contains?`/`member?`) and the scalar bridges that reinterpret
a string as another type (`string->number`/`number->string`, `string->symbol`/`symbol->string`).
`join` is `string/join` (it reduces a collection *into a string*); `string->number` stays bare (it
produces a number, and pairs with `number->string`). `std/text.blsp` is deleted, its `fill` folded
in as `string/fill`.

**Mechanism, no boot-loader surgery.** `string` is an ordinary `embedded_module!`, so hive's
source-parsing `docbuild` and every doc tool see a normal module file and group its functions. The
13 kernel primitives are re-registered *directly* under their `string/…` names (`string/length`,
`string/substring`, `string/split`, `string/upper`, …) — the builtin is the canonical name, no
wrapper. The prelude's own `get`/`path-*`/`fmt`/`doc-search` helpers reference `string/char-at`
etc. by late binding; since boot's namespace-resolve is a no-op for the root prelude (so those refs
don't auto-require), the prelude loads the module with one explicit `(require-one 'string)` right
after the loader is defined — making `string` always-present yet still `(:use string)`-able.

**Two mechanism facts worth recording.** (1) Bare names auto-qualify to `<ns>/name` only for
**def-heads the file declares** (the forward-ref pre-scan) — a *builtin* registered as `string/length`
is not a def-head, so inside `string.blsp` the renamed builtins (`length`/`substring`/`split`/`upper`/
`lower`) must be written qualified; the module's own sibling fns stay bare. (2) A call-position sweep
(`(name` → `(string/name`) is self-checking when it runs over Rust as well: the one false hit was a
Cranelift `Block` variable named `join` in `jit_lower/prim.rs`, and it failed to compile rather than
corrupting anything silently.

**Correction to an earlier note:** the "make `number->string` an extensible ability" idea is already
served — the prelude carries an always-on `Display`/`Inspect` protocol (ADR-168/172) dispatching on
the first arg's identity, so any type renders itself through that. No new protocol; the bridges stay
bare.

Scope: ~915 call sites across `std/`+`tests/`+`examples/`+Rust-embedded Brood, plus `sigs.rs`,
`PRIMITIVE_DOCS`, the `doc-catalog` (public entries renamed, private helpers dropped per ADR-227),
`docs/language.md`, and every `../*` sibling project. See ADR-230.


## 2026-08-18 — Stdlib namespacing, stage 6: the `file` library module (ADR-231)

Same treatment for the filesystem surface, mirroring stage 5. The 18 kernel fs primitives are
re-registered directly under `file/*` — `file/slurp`, `file/slurp-bytes`, `file/spit`,
`file/spit-append`, `file/spit-bytes`, `file/spit-private`, `file/exists?`, `file/dir?`,
`file/stat`, `file/mtime`, `file/size`, `file/ls`, `file/mkdir`, `file/rm`, `file/rmdir`,
`file/rename`, `file/cp`, `file/cwd` — no bare forwarders (greenfield). The old
verb-noun/adjective spellings are gone: `list-dir`→`file/ls`, `make-dir`→`file/mkdir`,
`delete-file`→`file/rm`, `delete-dir`→`file/rmdir`, `rename-file`→`file/rename`,
`copy-file`→`file/cp`, `file-exists?`→`file/exists?`, `file-mtime`→`file/mtime`,
`file-size`→`file/size`. `std/file.blsp`'s policy layer keeps its bare module names
(`read-lines`/`write-lines`/`list-files`/`list-dirs`/`walk-files`/`path-extension`/`path-stem`);
its `file?` predicate was freed and reintroduced as `regular?` (the `file/` prefix would have
read as "a file that is a file").

**The boundary rule: `file/*` = "the operation touches the filesystem"** (whole-file and byte
I/O, metadata/stat, directory listing and mutation, the cwd). **Bare** stays for the pure
path-*string* helpers in the prelude — `path-join`, `path-basename`, `parent-dir`,
`path-absolute`, `temp-path` — which manipulate a path as text and never call a syscall. Same
mechanism as stage 5: `file` is an ordinary `embedded_module!`, the builtins are canonical under
their `file/…` names with no wrapper, and inside `file.blsp` the renamed builtins are written
qualified (a builtin registered as `file/slurp` is not a def-head, so the forward-ref pre-scan
won't auto-qualify a bare `slurp`).

Two stragglers the call-position sweep did not reach, fixed here: the checker's guard-effect
list (`guard_effects.rs` still named `slurp`/`spit`/`spit-append` as effectful-in-guard), and
the user-facing `format!("<op>: {path}: {e}")` error prefixes in `io.rs`/`system.rs` (they name
the primitive, so they must track the rename — e.g. a failing read now reads `file/slurp: …`).
Living-doc references in `tooling.md`/`module-index.md`/`node-connect.md` updated too; the dated
historical entries in `decisions.md` and `devlog-archive.md` are left as the record of what those
primitives were called at the time. See ADR-231.


## 2026-08-19 — Bump to 0.4.0: the stdlib-namespacing renames are a compatibility boundary

The `string/*` (ADR-230) and `file/*` (ADR-231) renames removed the old bare/`string-`/`file-`
primitive names — a hard break for any published package that called them. They shipped while the
workspace version still read `0.3.11`, so `(brood-version)` reported the *same* number before and
after the break, and a package's `:brood` runtime-version constraint (ADR-209) had no way to gate
it: `:brood ">= 0.3.11"` was satisfied by a brood that predated the renames, and the only symptom of
running on the wrong one was a pile of `unbound symbol` errors deep in a dependency. (That is
exactly how the hive deploy silently pinned an incompatible brood.)

Bump the workspace version **0.3.11 → 0.4.0** so post-rename brood reports a strictly higher number,
and the ecosystem's packages now declare `:brood ">= 0.4.0"` — a real gate that fails setup with a
clear message instead of a cryptic unbound-symbol cascade. This is the first version bump used to
*mark* a break rather than just accrue features; the lesson is to bump on the break, not after it
bites. (Greenfield still means we break freely — 0.4.0 is a signpost, not a stability promise.)


## 2026-08-19 — The dependency resolver is now brood-version aware (ADR-209)

The root-project `:brood` gate was only ever checked at setup on the *root* manifest — the resolver
picked dependency versions with no idea what runtime any of them needed. So the newest release of a
dep could require a brood the running one didn't have, and you'd learn that only as `unbound symbol`
errors deep inside it, not as a resolution failure.

Now `:brood` rides through the whole registry path. Publish sends it in the release metadata
envelope; hive stores it in a new `releases.brood` column (idempotent `ADD COLUMN`, NULL =
unconstrained) and serves it in the `/releases` resolution slice; and the resolver's pure provider
**filters candidate releases to the ones the running brood satisfies** — so a version the runtime
can't load is never selected. When a package's releases are *all* incompatible, a pre-solve pass
raises a clear, brood-specific error ("no published release of X is compatible with the brood you
are running (0.4.0) — X's releases require Brood >= 0.5.0 — upgrade brood, or depend on an older X")
instead of the backtracker's opaque "no version satisfies". Backward compatible: a release with no
stored `:brood` (everything published before this) reads as unconstrained, so existing resolution is
unchanged — verified by resolving `store 0.2.2` off the live registry, which serves no `:brood`.

Bootstrap note: hive *is* one of the packages, so the first publish of the migrated deps necessarily
lands on the pre-feature registry (their `:brood` is sent but not stored); the resolver serves it for
every release published once the upgraded hive is live.


## 2026-08-19 — Release bundles: root a dependency module only when its name collides

A `nest release` bundle embeds every dependency module under a package-rooted key (`store/repo`,
ADR-070). The boot loader (`run-bundle` → `require-force-in`) then loaded each one UNDER its
package context, so its `(defmodule repo)` bound `store/repo/…`. But a real app references a dep
module by its BARE name — hive's `(:alias repo)`, a bare `web/router/through-handler` — exactly as
it does in a dev run, where a dep module is found by basename on the load-path and binds bare. So
the bundle bound `store/repo/*` while every consumer (and dev) referenced `repo/*`: hive's bundle
crash-looped on `require: cannot find module 'repo'`, then on the next dep once that was patched.
Rooting was **all-or-nothing** and mismatched dev.

Rooting exists for exactly one reason — two dependencies that ship a module of the SAME name
(`alpha/parser` + `beta/parser`) must stay distinct — so make it fire for exactly that:

- **`bundle-module-package` roots a key only when its short name collides** across packages
  (`bundle-short-collides?` over the rebuilt `*package-modules-of*`). A uniquely-named dep module
  loads at ROOT and binds bare (`repo`, `pool`, `wire/bytes`) — matching dev and matching how apps
  reference it. A colliding one stays rooted, and the consumer references it qualified
  (`alpha/parser`) — the ADR-070 guarantee, still green.
- **`bundle-resolve-bare` makes a bare require order-independent**: a bare `repo` / `pool` that
  isn't embedded directly resolves to the dependency's embedded rooted key, so a root module
  required before the dep whose module it uses still loads (and both names are marked provided, so
  the load-all loop never re-evaluates the source — no redefined-macro double-load).
- The full transitive dependency set the collision test needs is baked into the bundle manifest as
  `:bundled-packages` by `bundle-collect` (the direct `:dependencies` list omits transitive deps
  like `store`, which is exactly the one hive tripped on) and rebuilt into `*package-modules-of*`
  at boot.

Verified: the ADR-070 same-name-deps bundle test stays green, the hive bundle boots through to its
DB connection (was `cannot find module`), and the full suite is 4674/4674. This is a real dev/release
parity fix, not hive-specific — any app with a transitive or bare-referenced dependency was affected.

## 2026-08-19 — KI-36 reproduced and fixed: the deadline analysis was aimed at the wrong branch

A repeated-run gate before opening new work — three consecutive `make test` passes on the merged
tree at `692af1c5` — came back **992/992, 992/992, 992/992 (1 flaky)**. The flaky row was
`reconnect_watcher_heals_a_fallen_link`: **KI-36**, the repo's sole watch item, unreproduced since
2026-08-07 across 25 idle runs and 14 loaded ones. Third sighting, and the first with its output
captured, which is what settled it.

**It was never the nodedown stall the entry predicted.** The captured branch was
`TIMEOUT-no-pong`, and the cause is a race in the test's own round-2 script: B2 called
`node-start` (opening its listener, so A's watcher could connect and fire `[:nodeup]`) *before*
`register`ing `:echo`. A pings the registered name the instant it sees nodeup, with
`send-errors` already restored to nil — so a ping arriving in that gap is **silently dropped**, and
A waits out its 20 s pong deadline for a reply that was never queued. A longer deadline could not
have fixed it, which is worth noting because that is the reflex this class of failure invites.

**The old inference, and the flaw in it.** KI-36 argued that 22.6 s "can only be the nodedown
deadline (15 s + startup), not the nodeup deadline, which would have taken at least 45 s." But
reaching the *pong* branch never costs the 45 s nodeup deadline — nodeup fires promptly, since the
harness restarts B2 ~400 ms after B1 exits. Setup (~2.6 s) + the 20 s pong deadline ≈ 22.6 s, which
reproduces the original sighting's number exactly. The hunt was therefore aimed at a stall that did
not exist, which is the most plausible reason two years' worth of clean re-runs proved nothing.
**When a deadline analysis rules a branch out, check the time to *reach* the branch, not only the
branch's own timeout.**

**Fixed by ordering:** `register` now precedes `node-start`, so the name is live before any peer
can reach the node. Verified in both directions rather than by re-running until green — a 3 s delay
inserted between `node-start` and `register` fails **100%** with a byte-identical `TIMEOUT-no-pong`
pre-fix, and **passes with that delay retained** post-fix; then 10/10 under six equal-priority CPU
burners.

**Scope checked, deliberately not widened.** The same register-after-`node-start` ordering appears
at **16 sites** in `distribution.rs`, but only this one is exposed: everywhere else the peer is
spawned *after* `wait_until_listening`, and a fresh `brood` boot (~150 ms–4 s) always outlasts B's
spawn+register. This test is unique in having a peer already hot-looping a 100–400 ms backoff
`connect`. The other 15 are left alone.

**Two red herrings, and one of them was mine.** B2's `dist: incoming connection failed: failed to
fill whole buffer` is benign — `wait_until_listening` opens a bare `TcpStream::connect` and drops it
with no handshake bytes, so it appears on passing runs too. And KI-36's own recorded *method trap*
fired again, on me: a first verification loop classified on "did the success line appear" and
reported 8/12 failures, every one of which was 12 default-priority spinners starving a `nice -n 19`
test's child boots — a priority inversion I had created. Re-run with the output captured and
classified, the harsh condition yields only `wait_until_listening gave up` and **zero**
`TIMEOUT-no-pong`. A failure you cannot name is not evidence, in either direction.

## 2026-08-19 — Stdlib namespacing, stage 7: flat names for `proc/*` and `net/*` (ADR-233)

The `proc/*` and `net/*` module families were the last holdouts of the double-slash qualified
call — `proc/agent/start`, `net/http/get` — while the toolchain `std/tool/*` had long used bare,
single-segment module names (`test/run`, not `tool/test/run`). The directory prefix in those
names bought nothing: `module_to_require` already derives the module as everything before the
*last* slash, so `proc/` and `net/` were directory-as-namespace where the directory is really just
filing. Flattened them to match `tool/*`:

- `proc/gen → gen`, `proc/supervisor → supervisor`, `proc/agent → agent`
- `net/http → http`, `net/sse → sse`, `net/tcp → tcp`, `net/reconnect → reconnect`

Files stay under `std/proc/` and `std/net/`; only the `defmodule` and the `embedded_module!` key
changed. A qualified call is now `gen/spawn-server`, `agent/start`, `http/get` — one slash.

`std/editor/*` is the deliberate exception (ADR-233): a cohesive framework whose generic member
names (`buffer`, `ui`, `pane`, `ansi`) are scoped by the `editor/` qualifier and would collide/land-grab
bare — `editor/ansi` vs the top-level `std/ansi.blsp` is the tell. The rule is "the prefix must earn
its keep," not "one slash always."

**One test moved with the change**, and it was the right kind of failure: `namespace_test`'s
reserved-package-name cases asserted `net` and `proc` were reserved (as group prefixes owning
`tcp`/`gen`). They're no longer prefixes, so `reserved-package-name?` — derived from the embedded
module list — now reserves the *flat* names (`http`, `gen`, `agent`, …) instead, and `editor`
remains reserved as the one surviving group prefix. Updated the assertions to the new reality
(`http`/`gen` flat, `editor` group). Full proc/net suites green
(`agent 12`, `gen 21`, `supervisor 22`, `http 44`, `sse 20`, `tcp 34`, `tls 4`, `namespace 51`).

## 2026-08-19 — Stdlib placement review: path/file/hash/crypto homes + enum→seq (ADR-234)

Reviewed every module's public exports (79 modules) for "is this in the module a consumer would
look in?" Most cross-module name overlaps are legitimate (`path/join` vs `string/join` — same word,
different domain; namespacing keeps them apart), but four real misplacements fell out and are fixed:

- **`path` is now pure, `file` is now I/O.** Deleted `path/exists?`/`is-file?`/`is-dir?` (they did
  filesystem I/O by delegating to `file`) and `file/path-extension`/`path-stem` (pure path-string
  ops duplicating `path/extension`/`path/stem`). Each operation now has one home. `http` and the
  tests repointed; `file/regular?`/`file/dir?`/`file/exists?` are the I/O predicates.
- **`hash/bytes->hex` de-duplicated.** It re-implemented hex encoding that `encoding/hex-encode-bytes`
  already owns; now a private one-line delegation, no longer a public export.
- **UTF-8 codec moved `crypto` → `string`.** `crypto/str->bytes`/`bytes->str` were general string
  ops; they are `string/to-bytes`/`string/from-bytes` now (~30 call sites updated across crypto,
  tests; the round-trip test moved to strings_test).
- **`enum` → `seq`.** The module held sequence utilities (chunk-by, group-by, zip-with, …), not
  enum types. Renamed module + file (`std/seq.blsp`); the Rust checker's hardcoded `enum/dedupe`/
  `enum/interpose`/`enum/index-where` sigs updated too. The core `seq` function is untouched — a
  module qualifier is not a value binding.

All affected suites green (path/file/http/hash/crypto/strings/scram/uuid/seq/namespace). The
overlap completeness test (ADR-233) confirmed no new overlap slipped in. Next: ADR-235 makes the
overlap *list itself* unnecessary by resolving `:use` clashes lazily at point-of-use.

## 2026-08-19 — `(:use …)` clashes resolve lazily; the maintained overlap list is gone (ADR-235)

The `(:use …)` importer used to reject an overlapping name **eagerly**: `(:use sexp) (:use telemetry)`
failed to load just because both export `forward`, even if you only wanted non-overlapping names.
That eagerness is exactly why `namespace_test.blsp` had to pin `ns-known-overlaps` — a hand-curated
list of every std cross-module overlap (which drifted to 15-vs-25 and was rebuilt drift-proof only
this morning). The runtime already knew the overlaps; the list just re-encoded them in a second place.

Made resolution lazy. The import table value is now `ImportEntry::One(q) | Ambiguous([q…])`. On a
clash `refer_add` records `Ambiguous` instead of erroring; the resolver raises **only when the
ambiguous bare name is actually used**, at the use site, naming the candidates and the fixes:

```
`forward` is imported from more than one module (`sexp/forward` and `telemetry/forward`) —
the bare name is ambiguous. Qualify it (e.g. `sexp/forward`), or disambiguate the
`(:use …)` with `:only [...]` / `:exclude [...]` / an alias.
```

Plumbing mirrors the existing auto-require channel: `resolve_sym` records the ambiguous use to a
thread-local (`record_ambiguous`, armed only under `RECORDING`, so the read-only LSP path never
hard-errors), drained by `take_ambiguous_error` right after `resolve` in the macroexpand driver so
the error points at the form. The checker demotes the same way (`Heap::add_import_lazy`) and reports
it advisorily; `is_unbound` no longer double-flags an ambiguous name as "unbound".

Deleted `ns-known-overlaps`, its helpers, and the completeness test — overlaps self-report now, so
there is nothing to maintain in a second place (the whole point of the exercise). Replaced with four
behavioural cases (coexist / ambiguous-use-errors / qualified / `:exclude`), and rewrote
`modules_test.blsp`'s clash case from "eager hard error" to "lazy, only when used". `reserved-package-name?`
(package-vs-module) is a different clash and is unchanged. All resolution-sensitive suites green.

## 2026-08-19 (later) — KI-47: the tree-walker job was a memory threshold, not the tests it named

`main` is green on all five CI jobs at `c8dbf0ea` (run 32247618122) — the first fully green run
since the ADR-230/231 namespacing merge. Three jobs had been red: `rustfmt`, `breakage suite`, and
`differential (tree-walker)`. The first two were straightforward rot; the third took the session.

**rustfmt / nest format.** The rename commits lengthened call sites past the width limit and the
formatters were not re-run, so both halves of the format gate were red — three separate breaks in
one day (`066be566`, `242e688c` on the Rust side; `48a658ef`, `c8dbf0ea` on the Brood side). The
`make hooks` pre-push hook checks both in seconds and blocked two of my own pushes correctly; it is
cheaper than a red build each time.

**breakage.** The KI-42 pattern recurred **three times** — `string/*`, `file/*`, then `enum` → `seq`.
Each rename swept the directories a test run discovers (`std/`, `tests/`) and missed the ones nothing
discovers, so `breakage/` died on `unbound symbol: string-length`, then `substring`/`char-at`, then
`list->string`, then `enum/frequencies`. 108 call sites across 17 files. Two traps worth keeping:
`string->number`/`number->string`/`string->rope` survive the rename and must **not** be rewritten,
and rewriting `(name ` does **not** mean "a call" — in Lisp that also matches a `defn` parameter list
and a `let` binding pair, which is how a first pass produced `(defn echo-server-loop (file/ls
remaining)` out of a local listen-socket binding named `ls`.

**KI-47** is the interesting one, and it was mis-triaged three ways before it was understood — see
`known-issues.md` for the full record. Short version: the suite's process-wide allocation had reached
1.145 GB against a 1 GiB backstop; the three `adversarial_test.blsp` cases that "failed" were merely
the ones running when the line was crossed. Raised to 2 GiB soft / 3 GiB hard, which is what that cap
documents itself to be. Two fixes were built and discarded first — `gc-collect` (rested on a comment
ADR-061 had made false) and `hibernate` (a real 148× on a microbenchmark, wrong scope for a
process-wide cap).

**What this session did not resolve, and should not be read as resolving:** that cap was sized
against a ~240 MB suite peak, and the suite now peaks ~1 GB. The raise restores a ~2× margin instead
of the intended 4×, and whether the 4.8× growth is legitimate or a regression is **unmeasured**.

**A methodological note, because it dominated the day.** Six separate checks produced confident,
plausible, wrong output — and none of them errored: `rustfmt --check` piped to `/dev/null` (it writes
its diff to stderr, so dirty read as clean); a `(name ` regex matching binding positions; a stale
`nest` binary reporting `cannot find module 'string'` after `cargo clean` removed the current one; a
load-generator whose failures were all priority inversion I had created; a hand-rolled string-state
normalizer that desynced and declared reflow to be code changes; and a pre-push hook silently falling
back to a 0.3.11 binary. Every one looked authoritative. The pattern is the same one KI-36 and KI-39
embody, and it is the argument for ADR-232: make the *runtime* say something, rather than adding
another flag someone has to know to arm.

## 2026-08-19 — Stdlib prefix cleanup, exemplar: `queue`/`pq` (ADR-236)

Started dropping the redundant module-name prefix from std operation functions — `queue/queue-push`
→ `queue/push`, the smell being that the module qualifier already namespaces the name so repeating
it (`queue/queue-push`, `stream/stream-map`) is noise. `set/union`/`json/decode` were already the
house style. Carve-outs: `defrecord`'s `-field` accessors stay (CL `defstruct` idiom); the type
constructor/predicate keep the type name (`queue/queue`, `queue/queue?`, like `set/set`).

`queue` held two types, so it split: module `queue` (FIFO `push`/`pop`/`peek`/`new`/`empty?`/`size`/
`to-list`/`from-list`) and new module `pq` (`insert`/`pop`/`peek`/`peek-priority`/…), same-shaped
files (`std/queue.blsp` + `std/pq.blsp`, both embedded). `pq/empty?`'s body reaches core `empty?` as
`/empty?` so the un-prefixed name can't self-recurse. Both are reserved package names now. queue_test
repointed (27 pass); Display/Inspect verified (`#<queue 1 front=5>`, `#<pq 1 min=1>`). Next modules:
`stream`, `version`, `uuid`, `log`, `http`, `sse`, `tcp`, `wasm`, `multimap`, `mcp`, and the resolver
`pg-*` privacy cleanup — one commit each.

## 2026-08-19 — Prelude split into std/prelude/*.blsp (organizational)

The prelude was one 5779-line file. It can't become modules — it defines the bare root
namespace (everything usable without `(:use …)`), and a `defmodule` file would qualify its
names — so it's split into **9 numbered bare-root files** (`std/prelude/00-core.blsp` …
`80-tools.blsp`) at section boundaries and concatenated **in order** into the `PRELUDE`
const via `concat!(include_str!(…), …)`. Evaluation order is load-bearing (macros before
use, forward refs), so the `concat!` list in lib.rs is the authoritative order; the concatenation was verified
byte-identical to the former single file before wiring it up. `PRELUDE` is now a `pub const`
(one source of truth — `crates/nest/src/mcp.rs`'s `brood://prelude` resource uses it too
instead of re-`include_str!`ing the removed file). Runtime behaviour, source positions, and
the materialized `~/.cache/brood/prelude.blsp` LSP copy are unchanged. `nest format`
normalized the split files (trailing blank at each boundary); boot + maps/strings/namespace
suites green.

## 2026-08-19 — Prefix rollout, long tail: io/task/reload/protocol/gen/hash/format/package/telemetry/explain (ADR-236)

Finished the redundant-prefix cleanup across the remaining modules. Only PUBLIC functions were
de-prefixed — private `defn- mod-*` helpers keep their prefix (never exported, so no user-visible
`mod/mod-helper` doubling, and renaming ~400 of them across project/package would be pure churn).
Clean renames (no core collision): io-write→io/write, task-latest→task/latest,
reload-on-change→reload/on-change, protocol-ops→protocol/ops, gen-call→gen/call, hash-string→
hash/string, all of format/package/telemetry's public surface, explain-error→explain/error. A
boundary lookahead `(?![-\w?!])` protected prefixes-of-private-names (format-source vs
format-source-glue).

**"The prefix earns its keep" — modules deliberately NOT de-prefixed.** A de-prefixed name that
shadows a core MACRO or a HOF-value breaks in ways call-site `/escape` can't catch (a macro in
head position; `min`/`max`/`apply` passed as *values* to `reduce`/`apply`): fuzzy-match/filter
(`match` is a macro), stats-max/min (`reduce min`), project-apply (`apply`). These keep their
prefix, same call as the net modules (tcp/sse/http, whose prefix matches the kernel's bare
`tcp-*` socket builtins). The prefix there is disambiguation, not noise.

**Prelude shrink (B) assessed and declined.** Moving `random` (rand/rand-int/shuffle/sample) out
of the prelude into a module would touch ~184 bare call sites and is a UX regression — bare
`rand-int` is idiomatic and correct, and these are Brood defns, not core *semantics* (the "small
core" rule is about special forms/evaluator, which these aren't). `spy` is a bare debug
convenience by design. So the prelude is left as-is; the split (A) stands, the shrink (B) has no
clear win.

## 2026-08-19 — Forced through the collision keeps: stats, fuzzy, tcp, sse (ADR-236)

Revisited the "prefix earns its keep" modules and de-prefixed the ones that could be done
safely, leaving only the genuinely-unforceable:
- **stats** — stats-max/min → stats/max, stats/min; the four core min/max uses (calls and
  `reduce` value-position) are `/`-escaped.
- **fuzzy** — fuzzy-match/filter → fuzzy/match, fuzzy/filter; qualified-only, since a bare
  `(:use fuzzy)` would shadow the `match` MACRO.
- **tcp** — tcp/drain, tcp/read-n, tcp/read-until; guarded the kernel builtins (tcp-close,
  tcp-send) and the :tcp-* message keywords (renaming those deadlocked the receive loop).
- **sse** — sse/headers, sse/frame, sse/connect, and sse-send → **sse/emit** (not sse/send —
  `send` is the core message primitive; naming a function `send` hung sse_test via `:use sse`).

**Still kept (justified):** `http` — `get` is THE map accessor, used 25× inside http, and both
`sse` and `http_test` `(:use http)`, so `http-get`→`get` would shadow it catastrophically;
`project-apply` (`apply` core, ~12 internal uses); the `defrecord` `-field` accessors (CL idiom);
and every module's private `defn- mod-*` helpers (never exported). The lesson across the whole
rollout: drop the prefix, except where the bare name is a core MACRO, a HOF-value, or a pervasive
accessor/primitive the module itself leans on — there the prefix is disambiguation, not noise.

## 2026-08-19 — Going-live namespace policy recorded (ADR-237)

Consolidated the "how does the stdlib grow post-1.0 without clashing" decision into ADR-237. The
enforcement already exists (the three `reserved-package-name?` seams: publish / add-fetch-resolve /
require), so this is the policy write-up + the going-live commitment: the unprefixed root is
stdlib-owned; a dependency's local name can't be a stdlib name, so stdlib namespaces are
**un-shadowable**; therefore there is deliberately **no** `/enum/foo` namespace escape (nothing to
disambiguate, nothing to cargo-cult); `/name` stays only for the irreducible builtin shadow (a
library like `stats` defining `min`), with a future `nest check` redundant-escape lint to keep it
honest; and new stdlib names are additive + clash-recoverable (a grandfathered same-named dep errors
loudly and locally, never a silent shadow). Stronger than Elixir's convention-only stance.

## 2026-08-19 — Release 0.5.0: the stdlib-naming compatibility boundary

Bumped to 0.5.0 — this release is the compatibility boundary for the session's stdlib renames
(proc/* + net/* flattened to bare, enum→seq, path/file/hash/crypto placement, the ADR-236 prefix
rollout: queue/push, version/compare, uuid/v4, log/info, multimap/assoc, stream/map, tcp/drain,
sse/emit, …). Installed and used to migrate the local project ecosystem (bedit, hatch, chat, life,
terminal, pong, s3, store, mylife, hive, …) to the new names — each `nest check`-clean and green.

## 2026-08-19 — Green the tree after the ADR-236 sweep: fixture damage, a prelude-split guard, a pq bench

Post-pull verification of the prefix rollout + prelude split. `make test` was **red**: 994 tests,
1 failure — `brood-lsp workspace_symbols::tests::subsequence_matching`.

**The bug: a rename sweep that rewrote data, not references.** `matches` is a generic
case-insensitive subsequence test; it knows nothing about Brood names, and its test used
`"format-source"` purely as *fixture data*. The `format-source`→`format/source` sweep rewrote those
string literals to `"source"`, which silently made every positive assertion unsatisfiable
(`matches("fs", "source")` — no `f`). Restored to `"format/source"` (a live, realistic symbol shape
where all six assertions hold) with a comment marking the literals as fixture, not references. The
doc example above it (`fl0` matches … via f…o…) was rewritten too, and had a pre-existing typo —
`l` never appears in `format-source` — so it now reads `fso` … via f…s…o.

**Same sweep, same class, elsewhere.** The regex also ran through comments, doc comments, and
string literals, stripping the prefix off names in prose to leave bare fragments: `hash-string is
djb2` → "string is djb2", `explain-error` → "error", `gen-call` → "call", `reload-on-change` →
"on-change", `mcp-project-path` → "project-path", and a **user-facing error string** in
`introspect.rs` reading "source did not return a string". Prose restored to the new *qualified*
names (`hash/string`, `explain/error`, …), which is what the sweep should have produced there. It
also stripped the `mcp` tag out of four tests' scratch-directory names, dropping the disambiguator
that keeps their temp dirs distinct from other suites' — restored. (`*package-module-files*` →
`*module-files*` in `startup_image.rs` was checked and is a *correct* rename, left alone.)

**New guard: `crates/lisp/tests/prelude_manifest.rs`.** The split left the `concat!(include_str!…)`
list in `lib.rs` hand-written and unguarded — order is load-bearing, so it can't be derived — and the
omission mode is silent: add `std/prelude/foo.blsp`, forget the line, and the build stays green while
the file's `defn`s simply don't exist. Two cases, asserted against the `PRELUDE` const rather than by
grepping `lib.rs`: every `std/prelude/*.blsp`'s bytes appear in it, and its length is exactly the nine
files' total (catches a stale/duplicate include the containment check can't see). Verified by
sabotage — commenting out one `include_str!` fails both with the intended messages.

**Benchmarks: a `pq` module.** `queue` split into `queue` + `pq`, and the new module shipped with no
bench while its own docstring warns "O(n) insert / O(1) pop … swap to a heap if n gets large". Two
rows make that falsifiable, since `sorted-insert` walks only until it finds a lower-priority entry —
so cost is set by insertion ORDER, not n alone: `insert_descending_pop_all` (each insert stops at the
head) and `insert_ascending` (each walks the whole list). Measured, net of the ~1.6 ms fixed setup:
descending scales **10×** for 10× n (linear), ascending **~69×** (quadratic); they sit 1.9× apart at
n=100 and **26×** apart at n=1000. If those rows ever track each other, the sorted list has been
replaced or the walk broken. The rollout's own renamed rows (uuid/queue/multimap) re-run clean.

**Also fixed: the `explain/error` shadow.** `explain-error`→`explain/error` put a *core prelude*
name into a `:use`d module, so `nest check` warned at both `(:use explain)` sites and the tree lost
CLAUDE.md's "zero warnings across std/ + tests/" invariant. This is exactly the case ADR-236's own
rule keeps a prefix for ("a core MACRO, a HOF-value, or a pervasive accessor/primitive"), but the
shadow is a property of `:use`, not of the name — `explain/error` qualified reads well and is worth
keeping. So the `:use` was narrowed instead: `(:use explain :exclude [error])` at both sites, with
the 7 real call sites qualified (6 in `explain_test`, 1 in `std/tool/mcp.blsp`'s `explain-error-tool`).
`find-pattern` — the module's only other public name — stays bare. `nest check` is back to **zero
warnings**, and bare `error` in those files is the core raise again.

Counting sites needed care: `\(error\b` also matches `(error-shape`, which made `std/tool/mcp.blsp`
look like it had 13 bare raises when it has exactly one.

Tree green after, on top of 0.5.0: **996 tests, 996 passed, 3 skipped**; `nest check` **0 warnings**;
`cargo fmt --all --check` and `nest format` both clean.

## 2026-08-20 — A direct git/path pin overrides a transitive registry dep (ADR-238)

Extended the resolver's "root pin wins" rule across sources: a **direct** `:git`/`:path`/`:tarball`
dependency now suppresses a same-named **transitive registry** request, so the registry is never
consulted for that name (`resolve-deps` threads the direct-non-registry name set through
`package-resolve-loop`; the registry-collection branch drops an overridden name). Common all-registry
resolution is unchanged (empty override set). Guarded by an offline test in `package_test.blsp`
(git app → git dep declaring a registry sub-dep → resolves with zero registry contact); package_test
111/111, project_test 109/109. Motivation: a **bootstrap deadlock** — `hive` (the registry server)
resolved its own build deps *from the registry it serves*, so when hive's tarball route regressed and
404'd every package (including hive's own deps), hive could no longer be deployed to fix the outage.
ADR-238 is the minimal slice of the deferred per-dep-`[:patch]` override that lets hive pin its whole
closure (hatch, store-postgres, s3, transitive store) to git and build registry-independently — "hive
never uses hive". (Route regression itself fixed in the hive repo: a mangled `:version-tarball` /
`:version-docs` route string reverted to `:version/tarball` / `:version/docs`.)

## 2026-08-20 — The rename-sweep gap, closed: a gate for `stress/` + `scripts/fuzz/stress/`

Resuming after a crash, the tree was clean and six ADR-236 cleanup commits sat unpushed on top of
0.5.0. `--ff-only` reported "up to date" because local was *ahead*, not level — and the reflog showed
those commits had been rebased onto a newer `origin/main` **after** the devlog's green run, so the
combined tree was unverified. It re-verified green (996/996, rustfmt, `nest check` 0 warnings,
examples) and pushed.

**CI then went red on `clippy + test`, and the gap in my own verification was the interesting part.**
`make test` and `cargo build` do not compile `benches/`; only `cargo clippy --all-targets` does. The
prelude split had left `parse_prelude` `include_str!`ing the monolithic `std/prelude.blsp`, which no
longer exists. Fixed by pointing it at `brood::PRELUDE` (already `pub`) rather than a second
hand-written copy of the nine-file list — the bench now measures the same bytes the runtime uses and
inherits `prelude_manifest.rs`'s guarantee, so the next split cannot rot it.

**Then the same rot, one directory over — and grep under-counted it.** `stress/` and
`scripts/fuzz/stress/` are outside `make test`, `nest check`, the breakage suite and
`check-examples`. Grepping for ADR-236's names found 3 broken files; **running them found 8**,
because the earlier ADR-230 `string/*` wave had rotted them too:

- `string-length`, `string-split`, `string-repeat`, `string-span`, `string-span-until` (5 files)
- `format-source`, `gen-call`, `tcp-read-n`, `tcp-read-until` (3 files)
- some **double-prefixed** (`stream/stream-to-list`, `sse/sse-frames`) — the sweep qualified a name
  the module had also shortened

Two judgment calls a blind `s///` gets wrong. **`string/repeat` must stay qualified**: the prelude's
seq `repeat` takes its arguments in the OPPOSITE order, so a bare name would have silently computed
the wrong thing instead of failing — the dangerous shape, working-but-wrong rather than unbound. And
the first pass of my own regex renamed local helpers (`bench-tcp-read-until`) and printed row labels,
stripping the disambiguator out of text a human reads — **exactly the damage the original sweep did
to prose**, reproduced one step later by the person cleaning it up. Restored, with the labels
qualified (`tcp/read-until`) instead.

**A user-facing defect fell out of the hunt.** `(doc string/span)` ended "See also
`string-span-until`" and `string/span-until` ended "The complement of `string-span`" — both naming
symbols removed by ADR-230, so the cross-reference a reader types next is unbound. `display-width`
called itself "the width-aware counterpart to `string-length`". A stale name in `doc` is a broken
lookup, not untidy prose.

**The durable fix: `scripts/check-stress.sh` + `make check-stress`, gating every PR.** Modelled
directly on `check-examples.sh`, which exists for this same failure in `examples/`. It asserts **no
`unbound symbol`** rather than exit 0 — these are soaks and storms that legitimately never finish
under a gate, so their exit codes are environment noise while an unbound name never is — except
`stress/*_test.blsp`, which are real tests, cheap, and held to actually passing (78 cases). All 28
harnesses in **25 s**, so it runs on every PR: a rename goes red *before* it merges rather than one
red build later. **Verified by sabotage in both branches** (reintroduce `format-source` in a
harness, `string-length` in a test file → both FAIL, exit 1), and the annotate pipeline was tested
against KI-39's own shape — `grep | while` under `-eo pipefail` with no match still reaches the end
and prints nothing rather than killing the step.

The CI job is renamed **`examples + stress still run`**, since a job labelled "examples still run"
going red for a stress harness is the kind of misdirection that costs an hour.

**Method note, three sightings in one session.** Every wrong turn came from a check that reported
confidently and was not measuring what it claimed: `--ff-only` saying "up to date" about a branch
that was ahead; `echo $?` after a pipe reporting `head`'s status instead of `rustfmt`'s (the exact
trap 2026-08-19 recorded); a bare `nest check` at the repo root, which is not a `nest` project, exiting
0 without the file-list form CI uses; and a `jq '.conclusion // "pending"'` filter that printed every
in-flight job as concluded, because an unfinished job reports `""`, not `null`. Reproduce a gate with
the gate's own invocation.

## 2026-08-20 — KI-47's memory question, answered: legitimate growth, and the mechanism was wrong

The handoff's flagged "start here" was whether the suite's 240 MB → 1.145 GB (4.8×) was legitimate
or a regression. It is legitimate — and more usefully, **the comparison that produced the 4.8× was
measuring three different things**, and the mechanism KI-47 named is refuted.

**Module count is not the driver.** KI-47 blamed the stdlib split ("module loading costs memory
super-linearly here", citing RSS ≈ 45× source bytes). But 45× source bytes is linear in *source*, and
splitting a file into more modules barely changes source bytes. Tested directly, total source pinned
at ~40 KB while module count went 1 → 12 → 48 → 120: peak 11.23 → 12.59 → 15.13 → 18.59 MB. **120×
the modules costs 1.65×** — a marginal ~60 KB per module, so all 89 stdlib modules carry ~5.5 MB
against the +905 MB needing explanation. The 2026-08-06 module-load entry had been misread: its
quadratic was in **time** (`*features*`'s `member?` walk, fixed by ADR-216) and it explicitly found
memory *not* per-module ("10 big functions vs 100 small ones at equal line count costs the same or
more").

**The namespacing merge is not the cause.** `098a3316` (pre-ADR-230) built in a worktree and run
through the identical harness: **+4.4% on the VM arm, −11% on the tree-walker**. HEAD is *cheaper*
on the arm that actually went red. The merge crossed a threshold; it did not create the load — which
is what KI-47 itself suspected and could not check.

**Where the 4.8× came from.** The ~240 MB baseline is from 2026-05-30, the 1.145 GB from 2026-08-19:
different engine (tree-walker measured at **1.38×** the VM on the same suite and build), different
build (debug vs release), and three months of suite growth. Those compound past the multiplier with
no regression left to find.

**And it receded.** The same harness KI-47 used (debug, `BROOD_VM=0`, `brood_suite_passes`) now peaks
**726.7 and 757.9 MB** across two samples, against its 996.6 MB — −24% to −27%, back under the
*original* 1 GiB cap. The 2 GiB/3 GiB values stay anyway: ~2.6× margin at today's peak versus ~1.2×
if reverted. The number was right; the rationale was wrong, and `alloc.rs`'s comment now says so.

**Method notes.** Two traps hit in the space of an hour, both this repo's documented ones. A test
from the ADR-238 merge "failed" for me until the version banner gave it away — `brood 0.5.0
(25477eb4)`, a **stale binary**, and `std/` is embedded at build time, so the new test was running
against the old implementation (111/111 after a rebuild). And the first suite-memory number was taken
before that rebuild, so it had to be discarded. Separately: a single memory sample is not a
measurement here either — the two debug runs differ by 4.3%, which is small, but a lone 726.7 would
have been reported as a sharper drop than the data supports.

## 2026-08-20 — The source-position tables are not the memory target: 7.1%, not 18% (a negative result)

Picked as the next perf item on the strength of the 2026-08-06 module-load breakdown, which put the
two source-position side tables at **169 MB of a 933 MB load (18%) and 24% of load time** — the
largest single *attributable* item, with a dual memory+time payoff feeding the startup goal. Measured
on real code, both halves collapse.

**New measurement surface: `(pos-stats)`** (dev-tools only, beside `gc-stats`). It reports entry
count, capacity **and bytes** for the LOCAL `form_pos` and the runtime-shared `positions`, with bytes
derived from live *capacity* rather than entry count — hashbrown lays `(K,V)` slots out flat with one
control byte each, so `capacity * (size_of::<(K,V)>() + 1)` is the memory actually held. Deriving
from `len()` would have hidden the one real defect below.

**Memory, 38-module stdlib:** RUNTIME 14 012 entries / 14 336 cap / 473 KB; LOCAL 8 000 entries /
25 156 cap / 830 KB. **1.29 MB of an 18.23 MB load = 7.1%**, against the 18% implied. The gap is the
corpus: 2026-08-06 measured 1000-line *generated* modules at 1.15M entries, whose form density is
nothing like real source.

**Time:** an ablation build (`set_form_pos` returning early, the flag cached in a `OnceLock` so a
per-form `var_os` could not inflate *both* arms) loads the stdlib in **137 ms vs 146.5 ms** — a
**6.5% ceiling**, not 24%, against a **0.7%** base-vs-base noise floor measured first. And 6.5% is
the ceiling for deleting positions outright, which is not on the table: diagnostics,
`source-location` and the LSP all read them. A real optimisation buys a fraction of that. The
ablation switch was removed after measuring; it was never for merge.

**One real defect, deliberately not fixed.** The LOCAL `form_pos` holds its **high-water capacity**
for the process's life: the minor-collection path in `gc.rs` `retain`s in place and `HashMap` never
shrinks, so 8 000 live entries sat in 25 156 slots. But that `retain` is itself a deliberate time
fix — its comment records that taking and rebuilding the map was O(all positions recorded so far)
per minor rather than O(nursery) — so shrinking partly undoes a known win, and with safe hysteresis
(shrink at >3x, to 2x) it recovers only ~219 KB, about **1%** of load memory. Recorded in the
handoff rather than shipped.

**The lesson is the one this session keeps re-learning.** A documented number measured on a synthetic
corpus does not transfer to real code, and it had stood unchallenged as the obvious next optimisation.
This is the same failure as KI-47's "module count" mechanism earlier today — a plausible figure
re-quoted without re-measurement. Both were killed by measuring the actual workload. Note that the
first version of this session's own handoff entry re-quoted the 18%/24% figures as the recommended
next target; it has been corrected.
## 2026-08-20 — One conversion-naming convention: arrow `->` everywhere (ADR-239)

Unified conversion-function naming. The tree had three spellings for "convert an X":
the Scheme arrow `X->Y` (most functions), a `mod/to-Y`+`mod/from-Y` pair in a few
collection/type modules, and the ability ops `to-str` (`Display`) / `to-seq` (`Seqable`).
Standardized on the arrow: `->X` = result is X (`string/->bytes`, `stream/->vector`,
`pq/->list`), `X->` = source is X (`string/bytes->`, `queue/list->`) — the trailing arrow
is the symmetric mate, so a pair reads `string/->bytes`/`string/bytes->`. Two Rust string
primitives came along (`string/->codepoints`, `string/->graphemes`), and every *converting*
ability op became arrow-named: `Display` `->string`, `Seqable` `->seq`, `Temporal` `->iso`,
`JsonEncode` `->json` (the `->seq` op name is interned in `builtins/sequences.rs`, updated
there too). Non-conversion ops (`fetch`/`put`/`size`/`inspect`) keep their verb names. The
number-formatting primitive `to-fixed` → `->fixed` came along too — a formatter, not a
type conversion, renamed for a single consistent `->` prefix (kernel registration in
`builtins/`, ~20 call sites).

**Removed `symbol->string` and `number->string`.** `number->string` *was* `(str n)`;
`symbol->string` was `(name s)` + a strict guard, and `(str 'foo)` already gives `"foo"`.
The forward value→string direction is what the polymorphic `->string` (Display) op is for —
per-type behaviour via `impl`, not a family of `T->string` wrappers — so both were pure
redundancy. Call sites moved to `str` / `->string` / `name`. `string->symbol` stays (it
makes a symbol, not a string — the strict face of `symbol`). Curated checker sigs for the
two removed functions dropped from `types/check/sigs.rs`; the three `string/…list`/
`codepoints` sig keys renamed to match.

Mechanical but wide: ~140 sites across `std/`, `tests/`, the two benches, and the kernel.
Greenfield — no aliases. The module-qualified *shadows* of prelude globals (`stream/map`,
`multimap/get`, `string/repeat`) were deliberately left alone: intentional per-module
vocabularies, not a naming clash. Docs updated (`language.md`, `brood-for-claude.md`,
`types.md`, `ROADMAP.md`, `compute-frontier.md`); historical logs/ADRs/archives left as
written.

## 2026-08-20 — Kernel primitives stay flat (dash); a `/` is module-member syntax only

Tried to move the kernel primitive families to a slash namespace (`vector-ref` →
`vector/ref`, `table-put` → `table/put`, and a first attempt at `map/get`), and **reverted
all of it** — the idea fights the module system, in the same way each time:

**A `/` in a name means "module/member" throughout the toolchain**, in three places that a
kernel primitive is not a member of any module trips over:
1. `(:use mod)` refers a module's names *by prefix* (`system.rs` `%refer` enumerates every
   live `mod/…` global) — so `map/get`/`map/assoc`/`map/dissoc` get swept in and shadow the
   polymorphic prelude `get`/`assoc`/`dissoc` for every `(:use map)` consumer.
2. The project loader materialises a project by `require`-ing **every image section**, and
   sections are keyed by splitting names on `/` (`project-image-section-of`) — so a
   `vector/ref` reference makes a phantom `vector` section, and `nest test` dies on
   `require: cannot find module 'vector'` for *every* project that touches a vector.
3. Qualified-name rooting (ADR-070) treats `mod/name` as a reference into module `mod`.

`string/length` is the sole `/`-named primitive family and is fine **only because `string`
is a real module** (require-one succeeds, `(:use string)` refers its real names). So the
invariant: **kernel primitives are flat (dash) names; a `/` in a primitive name is allowed
only when the prefix is a real module-backed namespace.** `map`/`vector`/`table` are not
modules → dash. Enforced by `tests/primitive_naming.rs` so the next `foo/bar` primitive
fails at CI, not three deploys later. (Naming lesson for the reverse `sed`: `map/get` is a
substring of `multimap/get`; use the identifier-aware `scripts/ecosystem/blsp-rename`, not a
plain `s///`.)

Also fixed a latent gap in `runtime_collector`'s `drain_report_wires_through_the_scheduler`:
it drove `age → collect → assert` but skipped the `migrate_live_globals` step the real reclaim
cycle (`advance_runtime_multigen`) runs between aging and draining, so a *permanent* live global
(the boot `*features-loading*` tracker, an empty-map gen-0 handle) leaked into the "root clean of
gen 0" assertion depending on which slot boot's allocation ordering happened to leave it in. The
test now runs `migrate_live_globals(0)` before the assert, matching the documented
`age → migrate → drain` ordering — robust to any permanent live global, no runtime change.

## 2026-08-20 — Refreshing the perf standing: the menu was being ordered off a superseded table

Asked what was next on performance. The answer turned out not to need new measurement — it needed
**reconciling two documents that disagree**, one of which nobody was consulting.

`docs/runtime-frontier.md` is the option book (the *menu*); the benchmark repo's `FRONTIER.md`
states the *position*. The menu's "current standing" is **2026-07-30**; `FRONTIER.md` was
refreshed **2026-08-18**, after ADR-195, ADR-215, ADR-224/KI-40 and the lazy `FastLink` mirror all
landed. They disagree in ways that change the ordering:

| metric | menu (07-30) | position (08-18) |
|---|---|---|
| `spawn-live` | 2.42 s, 3.4× BEAM | **1.76 s, 2.5× BEAM** |
| live process | ~5.9 KB | ~5.5 KB |
| IC tables | 664 B → ~500 B | **896 B** — the older attribution was *"low by ~3×"* |

The IC number is the one that matters: at 896 B they are the **largest single attributed item on a
live process, bigger than the whole `Box<Process>`**, and `FRONTIER.md` makes the green-process
floor lever 1 "by elimination". My own measurement earlier the same day (~420–600 B by hibernate
differencing) was on a *bare parked* shell and understates the live case — worth recording as a
reminder that this quantity is workload-dependent and the shape must be quoted with the number.

**M2b corrected in two specific ways**, both verified in the source rather than argued:
`callee_bases` is process-specific by construction (`vm_call_ic_put` assigns it from
`vm_arm_block`, which takes `base = t.len()` — activation-order dependent), and the `arm` field
became a **process-local `ArmHandle`** in ADR-224/KI-40 on 2026-08-13, *after* the M2b entry was
written, precisely so the per-call clone avoids a shared refcount (3.19× on `pfib`). So the entry's
"the semantics of sharing are settled — only the read protocol is open" no longer holds: sharing
whole entries is blocked, and would need the entry split into shared-identity and process-local
halves, adding indirection to the hottest interpreted call path. `FRONTIER.md` independently names
the cheaper routes — **shrink `CallIcEntry`, or share entries for frozen callees only** — which is
where this should go.

**The banner is the actual fix.** The menu now says, at the top, that its table is superseded and
which document to read instead. Ordering work off that stale table is how three separate targets
were picked wrongly on this one day (KI-47's module-count mechanism, the position tables, and the
heap image that already shipped as ADR-218). Two stale MEMORY.md index lines were corrected the
same way — one claiming startup "needs a heap image", one claiming ~14× BEAM message latency where
the position says 2.7–3.3×.

**The recurring failure has a name now: a number that was true when written, re-quoted as a fact
about today.** Five instances in one session. The cheap defence is a dated pointer to the
authoritative source, which is what went in.

## 2026-08-20 — The "no default features" CI gate has never worked (and my own break proved it)

Adding `(pos-stats)` earlier today, I anchored an insertion on `pub(super) fn gc_stats(…)` — and
that function had a `#[cfg(feature = "dev-tools")]` attribute directly above it. The insertion
landed **between the attribute and the function**, so the attribute attached to `pos_stats` and
`gc_stats` was left ungated, calling the still-gated `gc_stats_map`. A lean build stopped
compiling. My mistake, and a reminder that an anchor immediately below an attribute is not a safe
place to insert.

**What matters more is that CI could not see it.** The `Check (no default features)` step ran
`cargo check --workspace --no-default-features`, and `crates/nest` depends on `brood` *without*
`default-features = false`. Cargo unifies features across a workspace build, so `dev-tools` came
back on for the shared `brood` lib and the check passed. Measured at the broken commit:

| command | result |
|---|---|
| `cargo check --workspace --no-default-features` (what CI ran) | **exit 0** |
| `cargo check -p brood --no-default-features` (the real thing) | **exit 101** |

So the break reached a developer's `make install` instead of CI. That step exists *specifically*
to keep the `#[cfg(feature = …)]` seams honest — it was added on 2026-07-19 after ungated
`crate::jit` refs broke the same configuration silently — which means **the guard has never
actually worked, in the only two cases it was there for.**

Fixed to `cargo check -p brood --no-default-features`, with the reasoning in the step's comment so
nobody "helpfully" restores `--workspace`. Verified by sabotage rather than by watching it pass:
re-removing the gating leaves the old command at exit 0 and takes the new one to exit 101.

The general shape, and the third instance today: **a gate that cannot fail is indistinguishable
from a gate that passes.** `check-stress` was added this morning for the same reason (two rename
waves rotted eight files with nothing watching), and the fuzz generators turned out to be emitting
programs that died on unbound symbols — reporting a checker false positive instead of comparing
engines, which also looked like working. Prefer proving a new gate red before trusting it green.

## 2026-08-24 — one definition site per primitive name; renames the compiler can check (ADR-240)

Started as fallout from the namespace moves and turned into the structural fix, because the
same bug shape kept reappearing: **a primitive's name is a string literal copy-pasted across
sites that agree only by string equality, so a rename that misses one fails silently at a
user's call site.** Three in one session — `%now-ns` unbound at boot (the registration was a
single-line `def()` a multi-line `%`-prefixing pass never matched, while its `PRIMITIVE_DOCS`
entry *was* rewritten); `%table-snapshot`/`%table-incr` unbound from the linmap rewrite (the
registration moved, the emitter in `macros.rs` and the PrimOp match in `ir.rs` did not); and
`table/new` reached from prelude code, where the `table/` module is not loaded.

Worth naming the near-miss: the `%table-*` breakage was hidden behind a **stale
`.brood/image.bin`**, so `nest check` reported the old world and the failures only appeared
after deleting the startup image. A cached boot image that survives a rename is its own
category of "gate that cannot fail".

Three layers, each verified by sabotage rather than by watching it pass:

1. **`PRIMITIVE_DOCS` is gone.** 391 `def()` calls now carry `name, arity, sig, params, doc,
   func` in one expression — the two parallel arrays that produced the `now-ns` desync no
   longer exist, and the drift-guard test that was supposed to catch it is deleted along with
   the drift it guarded. Done by script with a **snapshot oracle**: dump every native's
   `(name, params, doc)` before and after and require an identical diff. 390 rows, zero
   differences, so the merge is provably doc-preserving rather than plausibly so.
2. **Cross-file names come from `kw::` constants.** The table ops flow from `kw::TABLE_*`
   through all five sites (registration, `ir.rs`, `inline.rs`, `macros.rs`,
   `guard_effects.rs`). A rename is one line; the compiler flags the rest.
3. **A prelude-hygiene lint** walks the boot image's CST and rejects any qualified `mod/name`
   that is not a registered primitive or force-loaded. It immediately found a **shipped bug**:
   `(temp-path "x")` raised `unbound: rand/token` for anyone who had not required `rand`.
   Calibration mattered — the first version flagged `file/slurp` and friends, which are
   kernel primitives *named* with a slash and always bound; checking against the actual
   registration set cut 5 findings to the 1 real one.

**And the caller side: `nest rename` now goes through the CST.** `token-replace` is
boundary-aware but context-blind, which is why the rename waves rewrote comment prose ("the
offload pool" → "the proc/offload pool"), edited docstrings, and clobbered the prelude's own
`defn offload` head — the corruption I spent this session repairing by regenerating files from
`git show`. `codemod/cst-rename` reassembles each file from `parse-source`'s lossless CST and
rewrites only `:symbol` leaves, so a docstring, a `;` comment, and `(quote …)` data cannot be
touched by construction; `--refs-only`/`--defs-only` add the def-vs-reference distinction that
a text pass cannot express, and `--text` keeps the old behaviour when a rename really must
touch prose.

Losslessness is the property that matters, and asserting it on toy input was not enough: the
first version silently **deleted `~@`** from 24 stdlib files, because the CST spells
unquote-splice `:splice` and I had guessed `:unquote-splice`. Round-tripping all 320 in-repo
`.blsp` files through a no-op rename found it; an unknown wrapper kind now raises rather than
emitting its subtree bare. Same lesson as the `--workspace` gate two entries up: **prove the
new check red before trusting it green** — and prefer a property test over the whole corpus to
a handful of hand-written cases, since the cases you write are the ones you already thought of.

## 2026-08-24 (later) — the clash rule, and a duplicate that had been costing people (ADR-241)

Making `gen` core put fourteen library exports in conflict with a bare core name. The
blanket reading of "a library must not clash with core" would rename all fourteen; the
useful test turned out to be **"would a reasonable caller write `(:use this)`?"**

Best outcome of the pass: `supervisor/stop` did not need a new name, it needed deleting.
It was `(send sup [:$stop])`; core `stop` is `(send pid [:$stop])`. A supervisor *is* a
server process, so `(stop sup)` already worked — verified by starting a real supervisor,
calling core `stop`, and watching it leave `proc/list`, rather than by noting the sources
matched. The duplicate had already cost `bedit` a `(:use supervisor :exclude [stop])`.

Deleting it then exposed a quieter bug in hatch: `http/server` defines its **own**
`stop (port)`, so a bare `(stop sup)` inside that module called *itself* with a pid and
silently failed to tear the worker supervisor down. Its own test caught it. Fixed with the
root-qualified `(/stop sup)` — worth remembering that a module shadowing a core name
shadows it for *itself* first, which is where it will hurt.

The same "do we really need it" check retired `config/last-index-of` (an exact
reimplementation of the prelude's) and, in the other direction, confirmed
`version/compare` is emphatically NOT redundant: core `compare` gets every version case
wrong lexicographically (`"1.10.0" < "1.9.0"`, `"1.2" != "1.2.0"`). Kept.

Renames where the module really is used bare: `changeset/cast` -> `permit` (also the more
accurate name — a strong-parameters allowlist, not a conversion), `accounts/register` ->
`sign-up`, `docbuild/parse-source` -> `parse-module`, and store-postgres's `byte-at` ->
`octet-at`, which retired four `:exclude [byte-at]` workarounds and immediately caught a
test that had been silently relying on the shadow (it passed a *string* to what it thought
was the extension).

Also fixed hive's three long-standing failures. Two were stale expectations, but the third
was a real bug: `render-inline` returned a VECTOR of nodes in three branches, and the
template renderer reads a vector as an element and a list as a child sequence — so
`[:strong {} ["hi"]]` rendered as `<strong><hi></hi></strong>`. Every bold run on the
public changelog was broken.

### The same day, later: `nest run` was completely broken and nothing said so

`nest run <anything>` failed with `unbound symbol: getenv` — every invocation, no project
needed. `nest check` reported 0 warnings and the 4692-test in-language suite was green.

The name lives in a **Rust string**: `crates/nest/src/main.rs` builds the pre-run check as
`"(unless (= (getenv \"BROOD_NO_CHECK\") …"`. No checker can see that, and no `.blsp` tool
reads `crates/`. Ten more sites of the same shape turned up once looked for — executable
fixtures in `distribution.rs`, `reductions.rs`, `basic.rs`, `mcp_sandbox.rs`, plus
user-facing error hints in `dist.rs`/`mailbox.rs`/`eval/mod.rs` telling people to call
`(process-flag …)`, a function that no longer exists.

**`crates/nest/tests/run_main.rs` already existed and would have caught it on the first
run.** It was never run: the RAM ceiling on this machine rules out `make test`, which had
quietly become "run `cargo test -p brood --lib` and call it verified". The integration
tests are cheap — `-p nest --tests` is 20 s — and running them also caught the
`distribution.rs` fixture and two MCP tests, the latter a real API bug: an earlier perl
pass had renamed the **MCP tool** `process-info` to `proc/info`, and every other tool is
slash-free kebab-case because `/` is invalid in an MCP tool name. Reverted the tool name;
the Brood function stays `proc/info`.

A general "extract every Brood snippet embedded in Rust and resolve its free symbols" lint
was prototyped and **rejected**: ~162 candidates, overwhelmingly docstring prose,
deliberately-unbound fixtures (`no-such-fn`) and record names defined inside the snippet —
and the known-names set was itself unreliable (the dump missed `node/connect`, which
exists). A gate with that signal-to-noise gets ignored.

What shipped instead is `scripts/stale-names.sh`: grep the whole repo — `.rs`, `.md`,
quoted forms, string literals — for the specific names a wave just moved. It found all
eleven Rust sites in seconds and then **13 more `scripts/fuzz/stress/*.blsp`** that no
rename wave had ever touched. Sabotage-testing it caught a bug in the script itself: the
boundary class was written `[^…^-/]`, where `-` between `^` and `/` is a RANGE, so the
first version reported a clean tree over a deliberately reintroduced `getenv`. Third
instance this week of "a gate that cannot fail looks exactly like a gate that passes".

### Later still: the core reference said 613, and two thirds of that was not core

"Core is still too thick" turned out to be two different bugs plus three real moves.

**191 of the 613 were private.** `/reference/core` is the only doc page built from the LIVE
IMAGE instead of source, and its filter tested the NAME. A prelude `(defn- helper …)` still
binds a root global — privacy is a recorded fact (ADR-146), not a spelling — so the match
compiler's 40 `match-*` functions, every `receive-*`/`spy-*`/`defmodule-*` helper and the
`x`/`l` transducers were published as language vocabulary. One `(not (private? sym))`.

**22 more should have been private and weren't**, because the earlier pass matched
`^(defn name` and every definition nested in a `(check-allow …)` wrapper slipped through.
The bootstrap builders were the interesting case: `append-2`/`append-rev`/`append-empty?`
are `(def name (fn …))` because they run before `defn` *or* `def-` exists, so they cannot
use the private form at all — they take `%mark-private` once the macro layer is up.

Then the real moves: `dev/` for diagnostics (20), `%`-prefix for the ability registry (26),
`reflect/` for source tooling (18). The judgement that mattered was where to stop:
interactive introspection (`doc`, `arglist`, `bound?`, `apropos`, `macroexpand`) stays bare
because you type it at a REPL, and namespacing it taxes the commonest use to serve a rare
one. `check-allow` stays for the same reason — it is a pragma the checker matches out of
source text, so it reads as part of the code.

**And the junk drawer.** `category-of` falls back to `:other`, which is not an error, so an
uncatalogued name landed silently at the bottom of the reference. 41 entries — including
`stop`, `cast`, `call`, `spawn-server`, `defprocess` (uncategorised only because `gen`
moved into the prelude) and `tap`/`then`, which belong next to `->`. A reader scanning by
category simply never saw them. The category is gone and a test keeps it gone.

Two things this pass kept re-teaching. First, **Brood hides in Rust**: the checker
recognises the ability-registry names BY NAME, so `%`-prefixing them broke dispatch the
instant it built (`no impl of localize for :money`) — now `kw::` constants, per ADR-240.
`introspect.rs` builds `(source-location 'name)` as a string; `nest run` builds
`(check-file …)`. Second, **`nest rename` renames a token, not a binding**: it rewrote
`std/tool/workspace.blsp`'s own zero-arg `(check)` into `(reflect/check)`, caught by
`std_check_test` because the arity no longer matched. A module's own function sharing a
core name is exactly where the tool cannot tell the difference — the third time that shape
has bitten (hatch's `stop`, hive's `register`, now this).
## 2026-08-24 — A general review: five kernel bugs, and a `main` that was already red

A broad correctness review across the kernel (heap/GC, JIT/VM, scheduler/dist, builtins,
front-end) and `std/`. The headline is that **the default build computed wrong arithmetic on
the most idiomatic loop in the language**, and five green CI jobs did not know.

### The one that matters — KI-50

```lisp
(defn sum-down (n acc) (if (<= n 0) acc (sum-down (dec n) (+ acc n))))
(sum-down 200000 0)   ;; => 6251217600, want 20000100000
```

`std`'s `repeat` is this exact shape (`(repeat-acc (dec n) x (cons x acc))`), so
`(count (repeat 200000 :a))` returned **28033**. At 400 000 the corruption stopped being
silent and became `type error: -: expected number, got nil`, blaming `dec` — a prelude
function the caller never mis-called.

An arm that tiers has two frame layouts, and on a deopt the runtime must know which one the
live frame was built to, because each keeps its journal at its own slot. It decides by
**frame size**, deliberately, because `inline_installed` is flipped by `jit_tier` between the
sizing and the deopt (KI-26). Two facts then combined: `leaf_nslots =
d.nslots.max(nslots_total)` came out **exactly equal** to the small `nslots` when the spliced
callee needs no extra slot — `(dec n)` is that shape, measured `nslots=4, inline_nslots=4` —
and splicing *removes the residual `Call`*, which makes the derivation `pure_self` and hence
unjournalled, so the old "leaf **and** journalled?" predicate answered *no* and fell back to
the **small** layout's slot. In the spliced layout that slot is an ordinary local holding the
loop counter, so `jit_ckpt_resume` read `Int(165825)`, saw a positive integer, and decoded it
as a journal word: resume ip `n >> 16`, operand depth `n & 0xFFFF`.

Fixed in the "make the invariant true" direction rather than around it: one reserved slot so a
leaf layout is *strictly* larger, plus a `FrameLayout` enum so a frame reads only the journal
of the layout it was built to, and an unjournalled layout yields `None` instead of a foreign
slot. Guarded by `tests/jit_leaf_frame_layout_test.blsp`, sabotage-verified.

**Why nothing caught it, which is the transferable part.** The corruption needs *one long
activation*, not many calls — a loop calling the same function eleven times at 1 000 → 100 000
is entirely correct, while a single 180 000-iteration call is not. No `std` suite tests a large
input (grepping the eight non-`datetime` suites for `100000`/`200000`/`50000` returns nothing),
so it was invisible, and re-running the suite — the usual flake defence — could never help. The
missing thing is not a stress test but a **test dimension**: assert the same closed-form answer
at 10³/10⁵/10⁶ *and* across `BROOD_TIER` 0/1/2. That costs milliseconds and would have caught
this the day the leaf inliner defaulted on.

### The other four kernel bugs

- **KI-51** — `macroexpand_1` read a pair, then called the ADR-227 auto-`require`, which *loads
  a module* (arbitrary eval → GC), then dereferenced the stale handle. Debug tripped the epoch
  tripwire; **release silently walked relocated memory** and reported a 5-element form as
  `arity error: expected 1 argument, got 31309`. The neighbouring `macroexpand` loop argues in a
  comment that its handle "needs no slot" — correct when written, falsified by a later call.
- **KI-52** — `msg_roots`, the L1 delivered-message slot table, was a root set for the LOCAL
  collector in all three of its walks and for **no** RUNTIME walk, so a shared closure sent by
  handle to a parked receiver could have its code freed or compacted while still queued. The
  share-fn path's soundness note ("Phase 2 walks the whole local heap") is true for a handle
  embedded in copied data and false for the top-level value in the slot — which is what that
  path actually produces.
- **KI-53** — a `Pid` crosses the wire node-qualified; a `Ref` crosses as a bare `u64` and every
  node counts from 0, so two nodes mint colliding refs. A ref is what a pinned `receive` matches
  and what identifies a monitor. Mitigated with a per-runtime random prefix (node-qualified refs
  need a `Value` layout change, which the JIT ABI pins).
- **The receive-mark searched an unsorted queue.** `Envelope::seq`'s doc promised "the queue is
  always ordered by it — which is what lets a pinned `receive` binary-search", while
  `reinsert_candidate` reinserted at a *clamped* index and its own comment disclaimed exactly
  that guarantee. Reproduced end to end at the predicted values (`queue=[3,5,4,6]`,
  `partition_point → 3`, reply lost). Fixed by making reinsert seq-ordered, so the invariant is
  now total.

### `main` was already red — KI-54

Three tests fail on a pristine `28e8eeb2`, verified by building HEAD in a clean worktree rather
than inferred, and one commit caused all three: `7cb796f0`, making gen_server core and **bare**.
It reserved *too much* (ten generic names — `call`, `cast`, `stop` — became un-redefinable, which
is what broke `spawned_process_picks_up_redefinition`), reserved *too little* (`gen` fell out of
`(builtin-modules)`, un-reserving the package name), and bundled `gen.blsp` into `PRELUDE`
without declaring it in `EXTRA_PRELUDE_FILES`.

The last one is worth sitting with: that manifest test exists *precisely* so a file joining the
prelude is a decision rather than a drift. It worked. Nobody read the result. And
`docs/known-issues.md` asserted "No open items… `main` is green on all five CI jobs" the whole
time — a load-bearing claim that was false, which is worse than no claim. **Re-verify "green"
against a clean checkout before writing it down.**

Whether `call`/`cast`/`stop` *should* be permanently un-redefinable is left as a design call for
the owner — it follows correctly from ADR-166 plus the deliberate "bare" decision, but it is a
real cost that commit may not have priced.

### Also fixed

CRLF injection in the HTTP client and server (header values and the request target were spliced
raw); `gen`'s `call-timeout` leaking late replies; a throwing `:start` killing a whole supervisor
instead of counting against restart intensity; supervisor and agent client APIs hanging forever
against a dead server (they now use `gen/call`'s monitor+timeout discipline); a flat 100k-element
list overflowing the native stack in `scan_count_syms` (the reader's depth cap bounds *nesting*,
not length) and taking the runtime with it; `%isolate` killing bystander processes it did not own
and leaking its `defdyn`/`private` mark registries; `[:rect]` with an unbounded extent aborting
the process via the allocator; `%env-all` panicking on a non-UTF-8 environment variable and
*hanging* the runtime; `%os-cmd-stdin` deadlocking on any child that emits >64 KiB; a predictable
`%file-swap` temp path that a planted symlink turned into an arbitrary-file overwrite; several
i64 overflow panics and silent wrong answers in numeric/sequence builtins; quasiquote evaluating
set-literal elements instead of quoting them; symbols starting with `#`/`^` printing unreadably;
and a nested-set literal that bypassed the reader's depth cap entirely (`#{` × 300 000 → stack
overflow).

### A measurement worth keeping

**`assoc` on a vector is O(n), not an O(1) indexed replace** — the guide implied otherwise, and
that claim is why `shuffle` was written as a textbook Fisher–Yates and ran 16 s at n=8 000.
Measured, 20 000 `assoc`s: vector 486 ms / 2 386 ms / 7 523 ms at lengths 1 000 / 4 000 / 16 000;
map 28 ms / 17 ms at 1 000 / 16 000. Only the *map* `assoc` is cheap. `shuffle` rewritten over a
CHAMP index→item map is 206 ms (~78×). `docs/brood-for-claude.md` now carries the table.

## 2026-08-24 (later) — merging the review onto the namespacing waves; `main` is green again

The review branch landed onto an `origin/main` that had moved 17 commits (v0.9.0 + v0.10.0, the
bare core going 613 → 337 published names). Two things are worth recording.

**`origin/main` was itself red — 21 of 996.** Measured, not inferred: built `5952f5bd` in a clean
worktree and ran it. The namespacing waves moved names out from under a batch of tests
(`gc-collect` → `dev/gc-collect`, `vector-ref` → `seq/vector-ref`, `check` →
`reflect/check`, …) and nothing caught it. Combined with KI-54's three failures from `7cb796f0`,
`main` had been red for a while under a file asserting it was green.

All 19 remaining are now fixed by updating the *tests* to the new names. One deserves note because
it is exactly the trap in this kind of repair: `throw_and_catch` asserted error code **E0042** and
was getting **E0010**. The tempting "fix" is to update the expectation. That would have been
wrong — the test's Brood source called `(vector-ref [1 2 3] 7)` to provoke an out-of-range error,
and `vector-ref` had moved, so the call raised *unbound* before it could ever index. Fixing the
source to `seq/vector-ref` restores E0042 and the test proves what it always did. **A test that
starts reporting a different error code after a rename is usually telling you the rename broke its
setup, not that its expectation is stale.**

**A behaviour regression the merge exposed (KI-55, open).** Auto-require runs when a form is
*compiled*, so a closure shipped to another node — already compiled — never triggers it, and any
namespaced global in its body is unbound on the receiver. That guarantee used to hold for free
because these were bare prelude names. The failure surfaces on the receiving node, far from the
code that wrote the closure. Workaround is `(require-one 'reflect)` on the receiver; the real fix
is an auto-require hook on the deserialize path.

Also found while merging: `std/net/reconnect.blsp` was calling bare `now` (moved to `os/now`) — a
real break in shipped code, and the cause of two `cli::distribution` failures, not test rot. And
bare **`require` is not a bound name** at all (the callable is `require-one`), despite `CLAUDE.md`
and the ADR-065 note describing `(require 'test)`.

The `shuffle` conflict is the one that needed judgement rather than mechanics: upstream *moved*
`shuffle` into `seq/` while keeping the O(n²) body, and the helper it calls had been deleted by
this branch's Fisher–Yates rewrite — so the merged form would not even have resolved. Carried the
linear implementation and its private helpers into `std/seq.blsp`; re-verified post-merge at
n=20 000 (0.13 s, checked permutation).

**Both engines now pass 1012/1012.**

### The namespace waves: path/, bytes/, seq/, map/, os/, gui/ (337 -> 291)

Five more subsystems out of the core namespace. The mechanism that made `seq/` possible is
worth stating plainly, because I got it wrong first: **the prelude may reference a module's
names freely** — it already does so for 55 `string/…` names — because a qualified name
inside a function BODY is late-bound and resolves at CALL time. The module only has to load
before anything invokes it, which `(require-one 'string)` in tools.blsp already arranges.

The real constraint is narrower: **nothing may be called during boot before its module
loads** — and macro EXPANSION counts as during boot. Nine sequence names had to stay for
exactly that reason (`try` calls `but-last`, `defrecord` calls `mapcat`, `defmethod` calls
`mapv`, and those pull in the rest transitively). Computed as a transitive closure over
macro bodies rather than discovered one 40-second rebuild at a time.

Three failures worth remembering, none of which pointed at their cause:

- The mover picked a definition by glob order over all of `std/**`, so it cut `take-while`
  out of `stream`, `repeat` out of `string` and `zip` out of `zlib` — three modules' own
  functions that merely share a name with a core one. It searches the prelude only now.
- `hash-map` is EMITTED by the quasiquote lowering for `{…}` literals. Moving it broke
  `std/tool/test.blsp`, which surfaced as `describe`/`test`/`assert=` reading unbound
  across the suite — a failure that looks nothing like a map problem.
- The generated wrapper for a VARIADIC primitive forwarded `(first more)`, silently
  dropping every later argument. `bytes/concat` joined only its first iolist, and it
  surfaced as three tcp framed-read TIMEOUTS.

The prelude-hygiene lint from ADR-242 earned itself twice more, catching prelude code left
calling `bytes/->list` and `map/zipmap` with neither module force-loaded — latent unbounds
at a user call site, invisible to `nest check`.

Also: `%defseq` (the definer behind map/filter/mapcat) is now unpublished scaffolding, and
the `string->utf8-bytes` pair retired in favour of the `string/->bytes` pair it duplicated.

## 2026-08-25 — `brood --check` resolved `(:use M)` against the namespace, not the module

A single-file `brood --check` reported `unbound symbol: walk-files` for a file whose header
says `(defmodule codemod (:use file))` — and then, for good measure, `unused :use import: file`.
Both false; `walk-files` resolves at run time. The whole-project path (`nest check`) was clean,
which is what localised it.

**Root cause.** `types::check::setup_check_imports`' `ensure_loaded` asked "is this module already
loaded?" by testing whether *any* `M/…` global exists. `file` shares its namespace with 18 kernel
primitives (`file/slurp`, `file/ls`, …), so the answer was yes before `std/file.blsp` had ever been
read: the `(:use file)` imported the primitives and none of the module's own `defn`s
(`walk-files`, `read-lines`, `regular?`, `list-files`, `list-dirs`, `write-lines`). A whole-project
check never hit it because `project-ensure-loaded` loads every module first. `file` is the only
namespace a fresh image shares between primitives and an *unloaded* module (the other prefixes
present at boot — `map`, `seq`, `string` — are all loaded features), which is why it stayed hidden.

The test is now the **feature registry** (`*features*`), i.e. exactly what the runtime's
`require-one` consults, read from Rust with no eval. Erring toward "not loaded" is free —
`require-one` is idempotent — while erring toward "loaded" is what produced the false positives.

**The second warning was the dangerous one.** "Unbound name" and "this import contributes nothing"
are contradictory advice, and the second would have the reader delete an import their program
needs. The unused-`:use` lint now stands down whenever the unbound diagnostic fired on a bare
name: an import table that failed to resolve something the file references cannot prove any
import unused. It stays fully live for a file that resolves cleanly.

**Also fixed, different cause.** `std/prelude/tools.blsp`'s `impl-app?` reads `*project-name*` /
`*ns-package*` — ambients `defdyn`'d by `std/tool/project.blsp`, absent under a bare
`brood script.blsp` — behind `(bound? '…)` guards. The checker flagged them, i.e. warned on
correct code. A `(bound? 'name)` test in a top-level form now exempts that name for that form
(scoped per form, so a probe in one function can't silence a typo in another).

`brood --check` over all 104 `std/*.blsp` files: **zero warnings** (was 4). Regression coverage in
`crates/lisp/tests/check_use_imports.rs` (9 cases, including both "a real typo still warns" and
"a genuinely unused import still warns" — verified by sabotage: reverting the loader test fails
the two `file` cases) plus a `feature_loaded` unit test pinning both directions. Both engines
1025/1025.


## 2026-08-25 — KI-55: a shipped closure now brings its modules with it

Auto-require (ADR-227/229) fires when a form is **compiled**. A closure that crosses a node
boundary arrives already expanded and resolved, so nothing on the receiver ever inferred its
imports and `(fn () (reflect/form-pos …))` died there with `unbound symbol: reflect/form-pos`.
Before the v0.9.0/v0.10.0 namespacing waves those were bare prelude names, bound on every node by
construction — the guarantee moved without anyone noticing.

**The sender names them; the receiver loads them at the call site.** `closure_to_message` walks
the arms' forms it is already deep-copying, collects each *qualified* symbol outside a
`quote`/`quasiquote` subtree, and resolves the **distinct** ones through
`derive::module_to_require`, keeping only those bound as globals here — the filter that separates
a real reference from a qualified-looking symbol, and the reason an unloadable module on the far
side is a genuine error. The result is a `(module, probe)` pair per module in
`ClosureMsg::modules`, appended to the wire's `M_CLOSURE` record (protocol `BRD\x05` → `BRD\x06`;
a v5 peer would have read the count as the start of `captured`). On arrival, each module this
runtime lacks gets a guard woven into the rebuilt body —
`(if (bound? 'math/sqrt) nil (%try (fn () (require-one 'math)) (fn (e) (throw …))))` — built from
primitives and core special forms only, because a rebuilt body is never macroexpanded here.

**Not at deserialize time**, which is where it obviously belongs and is wrong twice: `from_message`
holds half-built values in unrooted Rust locals (the KI-51 shape), and the selective-`receive` scan
calls it **while holding the mailbox lock**, where `require-one`'s wait-for-another-loader `sleep`
— itself a `receive` — deadlocks. At the call site the load is ordinary evaluation, and its failure
is a catchable error naming the module, the reference, and the fact that the closure was shipped.

Cost, best-of-5 on a deliberately reference-dense closure (8 forms, 10 qualified refs, 20 000
serialisations): **51 → 75 ms**, +1.2 µs per closure, and zero for a same-runtime send (skipped
outright). Resolving once per distinct symbol instead of per occurrence is worth 4× here.

`source_positions_survive_a_cross_node_send` lost its `(require-one 'reflect)` workaround; a new
`a_shipped_closure_requires_its_modules_on_the_receiver` ships a body calling two modules node A
has never heard of (and a quoted `'math/not-a-real-name`, proving quoted data drags nothing
along). Both sabotage-verified against the original unbound errors. Suite 1026/1026.

## 2026-08-25 — ADR-243: a framework's client API is a module, not ten bare names

`7cb796f0` made the gen_server framework "core and bare" by concatenating `std/proc/gen.blsp`
into the `PRELUDE` bundle. A prelude name is *reserved* (ADR-166), so that one move seized ten
of the most generic identifiers in the language — `call`, `cast`, `stop`, `call-timeout`,
`code-change`, `spawn-server`, `spawn-server-link`, `spawn-server-named`, `gen-clause`,
`defprocess` — into the un-redefinable set.

The bill arrived the same day, as KI-54: `(def call …)` was refused outright (breaking
`basic::spawned_process_picks_up_redefinition`), `gen` dropped out of `(builtin-modules)` and
un-reserved the *package* name (failing `namespace_test`), and the file was bundled without being
declared (failing `prelude_manifest`). Worse, it propagated: ADR-241 was written under the new
premise and audited fourteen ecosystem exports for "clashes with core", renaming
`changeset/cast` and `accounts/register` — clashes that existed only because a framework had
taken the words.

`gen` is now an ordinary `embedded_module!` like its siblings `supervisor` and `agent`:
`gen/call`, `gen/cast`, `gen/spawn-server`, or bare inside a module that writes `(:use gen)`.
Nothing aliased, nothing shimmed — every caller in the tree updated.

The rule worth keeping: **being core does not entitle a framework to a bare global name.** The
prelude is the language (`map`, `first`, `send`, `spawn`); a framework's client API is a
vocabulary for talking to one kind of process, however central that kind is. `call`/`cast`/`stop`
are the clearest case — three of the most ordinary verbs in programming, none of which mean
"gen_server" outside this framework.

`PRELUDE_MODULES` and its `prelude_modules_are_bundled` honesty test existed only to re-reserve a
package name the bundling had orphaned; with `gen` back in `CORE_MODULES` the reservation is
automatic, so both are deleted and `EXTRA_PRELUDE_FILES` is back to one entry. Guard:
`tests/reserved_names_test.blsp` asserts the ten names are *free* while `gen/call` and friends
are reserved, and that `gen` is still a reserved package name.

## 2026-08-25 — ADR-244: a late reply now dies at the door, not in the mailbox

`gen/call-timeout` flushes anything already queued for its reply `ref` when it gives up. That is
only half the problem, and the missing half leaks. A reply the server posts *strictly after* the
deadline cannot be flushed — it has not been sent yet — so it lands carrying an unforgeable token
no `receive` of that process will ever match again: never consumed, growing the mailbox without
bound across a retry loop, and re-scanned by every later selective receive. A caller timing out
repeatedly against a slow-but-live server leaks one message per attempt.

Erlang answered this twice. Through OTP 23 the caller simply **exits** on a `gen_server:call`
timeout, which makes a late reply moot. OTP 24 added **process aliases**: the reply is addressed
to a one-shot alias, and once deactivated the VM drops later replies before they enter the
mailbox. We take OTP 24's answer, as a kernel mechanism rather than a library workaround.

**No new `Value` kind was needed** — which is the part worth internalising. A Brood `ref` is
already the unforgeable per-request token a reply carries, so deactivating the ref *is* the alias:
`(%ref-deactivate r)`, after which a message addressed to that ref is dropped **at delivery**,
before queueing, rather than filtered at receive time. "Addressed to" is the request/reply idiom's
own shape — a keyword-led vector whose second element is the ref (`[:reply r v]`,
`[:down mref pid reason]`) — which keeps the check O(1) and keeps a deactivated alias from
swallowing anything unrelated.

**Bounded by construction, free when unused.** The set is a fixed eight `u64`s stored inline in
`MailboxState`: it cannot allocate and cannot grow. An entry only has to outlive the one in-flight
reply its alias was minted for, so the common case reclaims itself — the late reply arrives, is
dropped, and the entry is forgotten in the same step. Overflow evicts the *oldest*, restoring
exactly the pre-alias behaviour for one long-abandoned ref rather than dropping anything wrong.
Both delivery paths already hold the mailbox lock, so the check is `dead_aliases.is_empty()` on a
field they have just touched: `pingpong` / `ring` / `spawn` / `spawn-live`, best-of-11 and pinned,
every row within its own noise floor.

A timed-out `gen/call` now leaves an *empty* mailbox rather than a flushed-so-far one, and the
same holds on the server-died path. `%ref-deactivate` is deliberately delivery-only — queued
messages are untouched, like `demonitor` without `flush` — so the flush and the alias compose
rather than overlap. Guard: `tests/ref_alias_test.blsp`, with both delivery paths
sabotage-verified separately, plus the eviction boundary and the end-to-end late-reply case.

## 2026-08-25 — KI-56: bounding the L1 copy, and why counting nodes was not a bound

The L1 local-send fast path copies a message straight into a **parked** receiver's heap while
holding that receiver's mailbox mutex. The lock is not incidental to the copy — it is what hands
us the parked process and therefore the exclusive `&mut` on its heap — so the hold is proportional
to the message, and a large one stalls every unrelated operation on that mailbox. Measured with a
`%mailbox-size` probe (a pure lock-acquire, zero message work, so a stall can only be lock wait):
p99 **1 106 µs at ~80 KB** and p50 **5 011 µs at ~1.6 MB**, against a wire path flat at 4–10 µs
across a 500× payload range, because its heavy work happens outside the lock.

ADR-245 takes the fix that leaves the lock discipline alone: **bound the copy** and decline past
the bound, falling through to the wire path. Moving the copy out of the lock was already rejected
on soundness — `shutdown_runtime_parked` reaps parked waiters, and during the window the process
is in neither place.

**The first cut was wrong in an instructive way.** Charging one unit per heap node as the walk
visits it *looks* like a bound, and it measured p99 **243 µs** at ~1.6 MB — 26× better than
uncapped, and still 45× worse than the wire path it was supposed to match. The reason is that
every container arm materialises before it descends: `src.vector(id).to_vec()` copies the entire
element array, and `map_entries` / `set_elems` / `list_to_vec` do the same. The O(n) cost was paid
under the lock and only *then* declined. A per-node charge bounds the recursion, not the work.

Each kind now declines **before** materialising — `len()` for a vector, `map_size` for a map
(doubled, for key and value) and a set, `range_len` for a range, and a bounded spine walk for a
cons list, the one kind with no O(1) length. That is worth 45× on its own:

| payload | uncapped p99 | per-node only | + early-out | wire p99 |
|---|---|---|---|---|
| ~78 KB | 89.7 µs | 23.1 µs | **3.2 µs** | 3.0 µs |
| ~1.6 MB | 1 875 µs | 242.5 µs | **5.4 µs** | 7.7 µs |
| ~3.9 MB | 13 029 µs | 576.7 µs | **5.7 µs** | 7.7 µs |

The ~8 KB row does not move at all, which is the point — below the budget nothing happens. The
A/B is unusually clean: `BROOD_L1_BUDGET` is a runtime flag, so both arms are the *same binary*
and nothing but the cap can differ.

On throughput the honest answer is **no resolvable difference**: capped read +2.9 % and +9.0 % at
10- and 100-element payloads (best-of-9, 4 clients), against a same-config base-vs-base spread of
6.4–9.4 % and 2.8–14.5 % on the same box. Neither clears `max(5 %, 2 × floor)`, so this is not
evidence of a cost and not evidence of none.

Two details worth keeping. An early-out returns before spending anything, so it marks the budget
negative explicitly — the *sign* is how `try_deliver_local` tells "too big" from "a value kind the
copier does not handle", and `BROOD_L1_STATS=1` now reports them separately. And the budget counts
nodes only because no node carries an unbounded payload: a string at or above
`SHARED_BLOB_THRESHOLD` crosses by handle, never memcpy'd, so only a sub-threshold string is ever
copied — a `const _` assertion fails the build if that threshold is ever raised out from under the
assumption. Writing the test is what found that the string-payload charge I had first added was
**dead code**.

KI-56's second instance — selective-receive's peek-in-place branch calling `from_message` under
the lock — stays open and unmeasured. This entry exists because a plausible claim about the send
side turned out half wrong; fixing the receive side blind would repeat the mistake.

## 2026-08-25 — the overflow check that overflowed, and 17 guards CI never ran

Two findings from finishing the GUI robustness sweep, both of the same shape: a guard that does
not guard.

**`%gui-icon!`'s overflow validation panicked on the case it was written for.** The check reads
`(w as u64) * (h as u64) * 4 == rgba.len() as u64`. For `w = h = 4294967295` the product fits u64
(1.8446744065e19) and the `× 4` does not — so under debug-assertions the *validation* panics with
`attempt to multiply with overflow`, on whatever thread it runs on, which for a GUI call is
exactly the uncatchable failure the validation existed to prevent. `checked_mul` throughout: an
overflow is `None`, which can never equal `Some(len)`, so it rejects. The test that caught it was
already in the tree — `dimensions whose product overflows do not wrap into a false match` — and
was failing the suite, killed rather than asserted.

**The `gui` renderer tests were never executed by CI.** `src/gui.rs`'s `render_robustness` cases —
the ones pinning that a wild `:scroll-region` offset cannot overflow the coordinate math — are
behind `--features gui`, which is opt-in. CI's clippy step builds `--all-features` and so
*compiles* them; the nextest step builds the default surface plus grammars and so runs everything
except them. Compiled by one step, executed by none. A dedicated CI step now runs
`cargo test -p brood --features gui,gui-gpu --lib gui` (17 cases); the system deps were already
installed in that job for clippy.

The GPU painter's cull got its guard the same way: the predicate was inline in a closure that
needs a live GL context, so it was extracted to `quad_visible` and tested directly. Writing those
cases corrected the change — an infinite *width* anchored on screen is not garbage to be culled,
it genuinely covers the viewport, costs one quad, and GL clips it. What must be culled is NaN
(every comparison false, which is why the predicate is phrased positively) and an infinity that
puts the quad off-screen.

## 2026-08-25 — KI-56's second site: the fix that turned out not to be a trade-off

ADR-245 bounded the L1 send-side copy and left KI-56's other instance recorded as "unmeasured,
not fixed blind": a selective-receive scan that has skipped the head cannot pop its candidate
(it may not match, and has to stay queued), so it rebuilds *in place* — `from_message` into this
heap, with the mailbox mutex held, once per candidate.

The reason given for not fixing it was a real-looking trade: pop/release/re-insert costs a lock
round-trip per candidate, and the comment in the code says peek-in-place exists precisely so that
"the scan's lock count stays ≤ the peek-only scheme's for every backlog length". Reading it again
in the light of the send-side result, that argument is **obsolete**. It predates the
leading-keyword filter, which rejects a message no clause could match on its tag *without
rebuilding it at all*. So pop/re-insert never applied to backlog length in the first place — only
to candidates that could actually match, which is a far smaller set. The design traded an
unbounded lock *hold* for a bounded lock *count*, and the send-side measurement had just shown
which of those hurts.

And the machinery was already there: the optimistic branch pops, releases, rebuilds, and calls
`reinsert_at_seq` on a non-match, with the seq-ordered re-insertion that survives a guard running
a nested consuming `receive`. The whole change is a condition on which route to take —
`message_fits`, a bounded, allocation-free walk of the `Message` tree that stops the moment the
count clears the budget, so the probe cannot become the stall it prevents.

**It was worse than the first site.** Same probe, and sent by name in both arms so the payload is
wire-format either way (the L1 path would otherwise hand a parked receiver a heap value that the
scan peeks without rebuilding, and the unfixed arm would look fine for the wrong reason):

| backlog × payload | peek p50 | peek p90 | peek p99 | popped p90 | popped p99 |
|---|---|---|---|---|---|
| 4 × 8 000 | 0.5 µs | 112 µs | 950 µs | **0.5 µs** | 12.2 µs |
| 4 × 40 000 | 0.7 µs | 952 µs | 2 179 µs | **0.9 µs** | 2.4 µs |
| 8 × 40 000 | **1 252 µs** | 2 800 µs | 5 569 µs | **1.5 µs** | 11.2 µs |

78× to 1 640×. The cost side is nothing: a scan over *small* candidates — which stay peeked in
both arms, so the only difference is the probe — reads +0.9 % / +0.4 % / +0.2 % at backlogs of
4 / 16 / 64 against a 0.6–1.0 % base-vs-base floor, and the ratio *shrinks* as the backlog grows
because the rebuild comes to dominate. (The first version of that harness measured 1 087 µs a
scan for every arm: with no matching message queued, `(after 1 :t)` was the entire measurement.
Parking a marker at the end of the backlog and re-sending it each round is what makes the scan,
rather than the timeout, the thing being timed.)

Two asymmetries with the send side are deliberate. A `Str` **is** charged by payload here —
`to_message` routes anything at or above `SHARED_BLOB_THRESHOLD` to a shared blob, but a
`Message` decoded from a *remote* node's wire frame carries whatever that encoder chose, so a
long inline string can arrive. And a shipped closure never fits at any size: rebuilding one
reconstructs code — arms, captured env, KI-55's woven module guards — which has no business
under a mailbox lock.

Guards: `tests/receive_under_lock_test.blsp`, five cases on same-tag backlogs (a different tag
would be rejected by the keyword filter and never reach the code under test — a test written
with `[:junk …]`/`[:want …]` would prove nothing here), sabotage-verified: dropping the popped
candidate instead of re-inserting it fails four of the five. Plus six `message_fits` unit cases,
including that the walk is bounded rather than exhaustive.

**KI-56 is now closed at both sites**, and the interaction is worth stating: ADR-245's send-side
cap pushes a large local message onto the wire path, i.e. from a `Payload::Local` (which the scan
peeks for free) to a `Payload::Wire` (which it must rebuild). Fixing only the send side would
have moved the stall from the sender to the receiver's own scan rather than removing it.

## 2026-08-25 — KI-57: the suite was 1061/1061 green with a use-after-GC in it

The GC-stress pass after ADR-245's receive-side work was meant to be a formality — the repo's
rule that one green run is not evidence for scheduler/mailbox code. It found a real one, on the
very tree that was about to be pushed:

```
use-after-GC: vector handle (nursery slot 3) is from epoch 12, but that generation is now
epoch 13 — a handle held across a collection without being re-rooted
  Heap::vector <- mailbox::collect_receive_tags <- scan_mailbox <- receive_match
```

`scan_mailbox` took the `receive` clauses' leading-keyword vector as a bare `Value` and decoded
it **lazily** — the decode sits inside the scan loop, skipped on a one-message mailbox. That
laziness is the whole bug: on any iteration after the first, the decode runs *after* a matcher
`apply`, which can collect at any eval depth (ADR-061) and relocate the vector. Handle captured
before, dereferenced after.

The fix was already written down eight lines above, for the value beside it. `matcher` is pushed
to the roots stack at `rbase+0` and re-read on every candidate, with a comment spelling out that
`apply` can collect and relocate it. `tags` needed exactly the same treatment and was passed
unrooted. It is now at `rbase+1`, re-read at the decode. Two lines, and the surrounding comment
had been describing the requirement the whole time.

In release there is no tripwire to fire: the decode reads whatever occupies the old slot, and
the tag filter then rejects a message it should have matched — a selective receive that silently
misses what it was waiting for.

**The more useful finding is why nothing caught it.** Collections are threshold-driven, so a
handle held across one is only caught if a collection happens to land inside the window. Nothing
in CI changes that — the breakage job arms the per-deref tripwire but still collects on a
threshold — and `BROOD_GC_STRESS=1`, which collects at *every* safepoint and makes the window a
certainty, had never run in CI at all. It was a thing you did by hand, during an investigation,
if you thought to.

So `make gcstress` runs the twelve process/mailbox-heavy test files under
`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`, and CI's breakage job runs it. Under a minute.

Two things about that gate are worth keeping, because both were nearly got wrong:

- **It is a DEBUG build, and that was verified rather than assumed.** The first version reused
  the breakage suite's `--release` + `-C debug-assertions=on`, which *sounds* strictly stronger.
  Run against the faithful pre-fix code it reported **all clean** — optimisation moves where
  allocations and safepoints land, and the collection stops falling inside the window. The debug
  build catches it every run. A gate that cannot fail on the bug it was written for is not a gate.
- **It was checked by reverting the real fix, not by a stand-in.** The first sabotage kept `tags`
  rooted and captured a stale copy of the handle — a *weaker* bug (the vector moves rather than
  possibly dying), and it reproduced only 1 run in 3. Reconstructing the actual pre-fix code gave
  9 in 10 and a deterministic red through the gate. When a sabotage under-reproduces, suspect the
  sabotage before concluding the gate works.

## 2026-08-25 — the first cross-language run on 0.11.0, and the inline a rename had retired

Installed the lean build and ran the full seven-language harness for the first time since
0.3.11 (2026-08-14). Same machine, so the columns are comparable. Brood's compute, 0.3.11 →
0.11.0: `sort` −28%, `pingpong` −22%, `spawn-live` −21%, `base64` −15%, `persistent-map` −14%,
`pipeline` −10%, `json` −9% — and **`sieve` +1055%**.

`sieve`'s program changed only `(table)` → `(table/new)`. Same algorithm. So the 11.6× was the
runtime, and the cause was a comment that had quietly become false:

> `resolve_prim3` — *"Only a **direct** native binding qualifies (its one member, `table-put`,
> has no prelude wrapper to follow)."*

True when it was written. The v0.9/v0.10 waves moved the table API into `std/table.blsp`, so
the head is now `table/put`, a closure whose whole body is `(%table-put t k v)`. The resolver
stopped matching and the call compiled to an ordinary `Call` in the hot arm — no
`PrimOp3::TablePut`, on the row that exists to benchmark it. Nothing errored, no test failed,
and `nest check` was clean, because the program was still *correct*.

What hid it is worth more than the fix. The **2-ary** `resolve_prim` already follows its
wrapper through `passthrough_arm`, so `table/has?` in the very same loop kept inlining. One IR
dump of `mark` shows both states at once — a `Prim2` and a `Call` three instructions apart:

```
before:  … GlobalIc Local Const Call  Pop Local Prim2SlotSlot SelfCall
after:   … GlobalIc Local Const Prim3 Pop Local Prim2SlotSlot SelfCall
```

The arm lowered to native either way, which is why none of the standing diagnostics fired: no
`[jit-bail]`, and the JIT-vs-no-JIT ratio check (the one that exposed nbody's silent bail)
sees a native loop in both cases. It was native; it just called out per element.

The fix mirrors the 2-ary wrapper-following into `resolve_prim3`, and inherits its safety
rather than adding any: `head` stays the original call head so a deopt dispatches the real
wrapper with bit-identical errors, and the existing epoch `guard` re-validates on every
`global_epoch` change. The identity argument map is **required, not applied** — `Node::Prim3`
has no permutation field, unlike `Node::Prim2`'s `map`, so a wrapper that reorders its
parameters must decline; inlining one would store the value under the wrong key. That is the
second half of the guard test.

`make ab` best-of-7 against the parent: **457 → 68 ms, −85.1%**, 29 other rows noise.

**The sweep's one flag was not real, and the way it dissolved is the reusable part.**
`spawn-live` read +5.4% against a 2.0% floor, which clears the gate. But that row executes no
table code at all — the only `std` user of `table/put` is `telemetry`, which it never loads —
so the change cannot reach it. A fixed-baseline control then showed why: base-vs-base on that
row measured a **7.5% floor**, and simply *reversing the order the arms ran in* moved the
verdict from "+8.9%, regressed" to "+4.5%, noise", with the new binary landing between the two
base samples. Running the arms in a fixed order puts every within-rep thermal trend on the last
one. FRONTIER already says a `spawn-live` movement is not a result without a fixed-baseline
A/B; the order effect is the mechanism behind that rule.

**The class is the finding.** This is the third rename to silently retire an inline — KI-44 was
the same shape on `sqrt`, and its own fix note names the requirement it broke ("a **bare** head
resolving to a **PRELUDE** closure"). An inline keyed on how a name is *spelled*, or on which
region it is *bound* in, is a performance cliff no test, checker or CI job can see, because the
program remains correct. The structural, wrapper-following resolutions survive a rename; the
direct-binding checks do not. Worth auditing the remaining ones before the next wave.

## 2026-08-25 — KI-59: the CI-friendly run mode reported failure for programs that worked

A suite run went red on `nest::cli_failure_reporting`, on an assertion that reads "a clean run
must exit 0". Standalone it passed eight times out of eight; the failing run was under full
suite load. Under twelve spinning cores it reproduced at about **one run in six**, and the
output named the bug immediately:

```
fine
[exit] :noproc
```

The program ran, printed its output, and the driver reported `:noproc` — then exited 1. The
wrapper `nest run --for` generates was `(%spawn …)` followed by `(monitor p)`: two steps. A
program that finishes in the window between them leaves nothing to monitor, so `monitor` fires
a **synthetic** `:noproc`, `(= :noproc :normal)` is false, and success reports as failure. The
narrower the program, the likelier it lands in the window — and `--for` is documented as the
CI-friendly way to exercise an app, which is exactly where a spurious red costs most.

The kernel already names this race one field over. `%spawn-link`'s docstring: "atomically links
the child to the caller before it runs (**no spawn->link :noproc race**)" (ADR-067). The link
half was solved; the monitor half kept the two-step form.

**Reading `:noproc` as success would have been the wrong fix**, and it is worth writing down
why, because it is the tempting one-liner. A program that *crashed* before the monitor attached
produces the identical `:noproc`. The comment beside this code records that `nest run --for 3s
boom.blsp` once printed a crash and reported success — the bug this wrapper exists to prevent.
Mapping `:noproc` to 0 restores it for fast-crashing programs.

So the fix is atomicity, and it needed no kernel change: `%spawn-link` + `trap-exit`. The link
is established before the child runs, so the real reason always arrives, and trapping turns it
into `[:EXIT pid reason]` rather than killing the driver — which is what `monitor` was there
for. 0/20 under the same load that failed before.

**One thing not to overclaim.** The extended test — an instant-exit program, asserting the
printed reason and not just the exit code — does **not** make this deterministic: with the fix
reverted it still fails only ~1 in 6 under load, no better than the original. What prevents the
bug is the structural change. The test is a tripwire, not a gate, and a single green run of it
proves nothing. That is worth saying because the natural move after fixing a flake is to point
at the now-passing test as the evidence, and here it is not.

## 2026-08-25 — the gate KI-58 prompted found a second dead inline on its first run

KI-58's write-up ended "worth auditing the remaining direct-binding checks before the next
wave". The audit turned out to be a test rather than a reading: every call-site inline is keyed
on what a head *resolves to*, and nothing asserted it still resolves — which is precisely how a
rename retires an inline in silence, since the program stays correct and the checker stays
clean.

`every_inlinable_head_still_reaches_its_primitive` is one table of the spellings a program
actually writes — `+`, `-`, `*`, `cons`, `max`, `min`, `nth`, `first`, `rest`, `nil?`, `pair?`,
`empty?`, `type-of`, `table/get`, `table/has?`, `table/put`, `math/sqrt` — asserted to reach
their primitives. It deliberately checks the user-facing names rather than the `%`-prefixed
natives: the natives are where the resolver bottoms out, so testing those would pass while every
real call site had stopped inlining.

It failed on its first run. **`table/get` does not resolve either** — different cause, same
class. Its wrapper is `(defn get (t k &optional default) (%table-get t k default))`, and an
`&optional` head is not a thin wrapper: it binds a default before forwarding, so a 2-arg
`(table/get t k)` has no passthrough for `resolve_prim` to follow. `table/has?` sitting beside
it — a plain 2-ary forward — never lost its inline, which is what kept the gap invisible, the
same asymmetry that hid `put`.

The fix is in Brood rather than the compiler: two arity clauses instead of one `&optional`
head, so the 2-arg arm is a pure forward. `%table-get` is `Arity::range(2, 3)` documented as
"default (nil if omitted)", so the 2-arg call is exactly equivalent and the existing resolver
handles it with no change.

Three instances of this class now (`sqrt`, `table/put`, `table/get`), each found by a different
accident — a dead benchmark row, a cross-language harness run, and a test written about the
previous one. The gate is the first thing that will find the fourth on the commit that causes it.

## 2026-08-25 — the stdlib had lost stderr, and only a checker test noticed

Merging `origin/main` (the `output moves to io/` wave) brought in three failing `nest` tests.
Checked out at `f390bd56` with none of this branch's work present, they fail there too — main
was red.

Two causes, one interesting. The dull one: two test fixtures still wrote bare `(print …)`,
which the wave made unbound. Worth noting anyway that two of them were the **argument-injection
payloads** in `complete.rs` — with `print` unbound a payload cannot print even if it *did*
execute, so those security tests had quietly become false passes.

The real one: `(io/print … :to *err*)` was writing to **stdout**, with a literal
` :to #<native %write-err>` appended, everywhere in the stdlib.

```
Building mf (1 file) :to #<native %write-err>
src/main.blsp:3:15: warning: unbound symbol: print :to #<native %write-err>
```

`split-target` only reads a trailing `:to <port>` as a destination when `port?` accepts it, and
`port?` is `(satisfies? 'Port x)` over `(impl Port :fn (write [f s] (f s)))` — "a bare 1-arg
sink fn is a port". `*err*` is `%write-err`: `(type-of *err*)` is **`:native`**, not `:fn`, so
the impl never matched, and the pair fell through as ordinary values. `log`, the test runner,
`supervisor`, `repl`, `telemetry` and `format` all lost stderr. `*out*` (`%write-out`) had the
identical defect and merely looked correct, because the fallback destination is stdout too.

The fix is one line — `(impl Port :native (write [f s] (f s)))`.

**What caught it is worth more than the fix.** Of the three red tests, two only tripped over the
renamed `print`. The one that actually pointed here was
`declared_sig_is_authoritative_cross_module`, which asserts on **warnings read from stderr** and
saw none. Reverting only the new impl confirms the split: that test fails, the other two pass.
A test that reads the stream it cares about found a stream-routing bug that every test asserting
on *stdout* was blind to — several of which were happily matching text that now carried a
` :to #<native %write-err>` suffix.

**The general lesson is about ability dispatch.** `impl` is keyed on identity, and `:fn` and
`:native` are two identities for things that are both "callable with one argument". An ability
meant to cover "a function" needs both impls, and nothing warns when it covers only one: the
result is a silently-declined dispatch with every binding present — the same failure shape the
stdimage boot-install note describes ("defines its bindings and evaluates NOTHING… the failure
type-checks perfectly clean").

## 2026-08-25 — the same one-line fix, arrived at twice

The `Port :native` fix above landed twice within the hour: here, from three red `nest` tests
after merging the `io/` wave, and upstream as `2b6b1672 fix(io): the ports the language ships
with were not ports`. Identical line, `(impl Port :native (write [f s] (f s)))`. Git merged both
additions without a conflict — they were in different places in the file — so the merged tree
briefly had **two** impls of the same op for the same identity, which is a last-wins collision
rather than an error. De-duplicated by hand, keeping upstream's comment.

Worth recording because the two routes to it found different halves of the story. This side had
"which call sites broke" (every `:to *err*` in the stdlib: log, the test runner, supervisor,
repl, telemetry, format) and "what surfaced it" (a checker test asserting on warnings read from
stderr saw none). Upstream had the sharper question — **why did every test pass?** — and its
answer: `with-err-str` rebinds `*err*` to a Brood closure, which *is* a `:fn`, so the whole
capture-based test suite only ever exercised the working case. The shipped default was the one
nothing tested.

That is the generalisable bit. A test that installs its own double to observe an effect is not
testing the default path, and for `*out*`/`*err*` the default path is the entire product. Both
comments are now in `std/io.blsp`; neither alone tells you to go look at the default.

## 2026-08-25 — the startup image did not work on the build that ships

Chasing KI-61's fix — make a boot module load cheap, which is what the stdlib image is for — the
first probe that actually *installed* the image found it broken:

```
error: require: cannot find module 'test'
```

The image is keyed on `stdlib-id`, the stdlib's content hash, so `brood`, `nest` and
`brood-lsp` from one tree share a single copy rather than writing three. That is right. What it
misses is that a shared key does not imply a shared *module set*: a lean runtime — `nest
release`, `make install INSTALL_FEATURES=RUN_FEATURES`, i.e. what ships — bakes in 88 modules
and no dev-tools, while `std/tool/project.blsp`'s recorded edges name `test`. `install` replays
those edges (it must: a restored module evaluates nothing, so without them `url` comes back
with no `path`), and replaying an edge to a module with no source poisons `require` for
everything after it.

Fixed by dropping, at install, any dep this binary cannot load. At **install** rather than at
build because the image may have been written by a different binary from the one reading it —
which is the whole point of sharing the key, so the reader is the only party that knows.

The number this unlocks was already claimed in the docstring and had simply never been reachable
on a shipping build. Release, best of 3: `require format` **62.0 → 12.8 ms** and `require
datetime` **3.3 → 0.39 ms**.

Two things worth keeping. First, it went unnoticed because the image is *built* by `make install`
but never *installed* — dead until something calls `stdimage/install`, which nothing in the
normal path does. A feature that is constructed and never exercised is not covered by "the suite
is green". Second, it would have wrecked the KI-61 replay investigation had it not surfaced
first: a boot-install experiment on a lean build would have produced a pile of failures with
nothing to do with the registrations it was meant to measure, and the obvious reading — "see,
131 failures, the note was right" — would have been wrong.

## 2026-08-25 — the benchmark "regressions" were mostly measurement, and one of them was real

The 0.12.0 harness run listed `spawn` +35%, `loop` +22%, `regex` +22%, `fib` +15% against 0.3.11.
Chasing them produced a more useful result than a culprit commit: **most of that list is
measurement, and what remains is one real effect nobody had named.**

**Old binaries cannot run today's rows.** `os/getenv` did not exist at `d5572d61`, so a bisect
across this range is structurally impossible without help. Two ways round it, both used: neutral
microbenchmarks in the stable subset, and mechanically down-migrated copies of the real programs
(`io/puts`→`println`, `string/->number`→`string->number`, `os/getenv`→`getenv`) so each binary
runs the same algorithm in the spelling it understands.

**Startup contaminates every row, and it is not evenly distributed.** `compute = wall − startup`,
with one startup figure per language — but the startup row is `(io/puts 0)`, which loads `io` and
not `os`/`string`, so it under-subtracts for every real row. Measured per binary with its own
startup, the compute rows are flat or better: `loop` 23.1 → 24.4, `fib` 15.8 → 15.3,
`spawn` 283.5 → 221.3 (**−22%**).

**The real effect: loading modules taxes JIT'd code.** An identical, allocation-free 20M loop,
timed by differencing two otherwise-identical programs so module-load cost cancels exactly:

| modules required first | 0.3.11 | 0.12.0 |
|---|---|---|
| 0 | 24.7 ms | 26.3 ms |
| 4 | 23.3 ms | 33.7 ms |
| tax | **−5.8%** | **+27.9%** |

JIT-only — `BROOD_NO_JIT=1` reads 942 → 939 ms, `BROOD_TIER=1` 949 → 949. The interpreter does
not care. Filed as KI-63; the mechanism is not identified, and it is none of shared arms, the
RUNTIME GC floor, or either inliner.

**Two traps, either of which would have produced a confident wrong answer.**

- *Pinning.* The same measurement reads **+68.2%** pinned against +27.9% unpinned, because the
  background JIT compiler competes for the pinned core and loading modules increases compilation
  volume. CLAUDE.md documents this for `make ab`; it applies to any hand-rolled measurement too.
  Both sides inflate, so a pinned reading exaggerates the regression as well as the absolutes.
- *Subtracting a startup row.* Differencing two programs is exact; subtracting a startup figure
  measured on a different program is not, and the error is systematic rather than noise.

And one suspect cleared by measurement rather than reasoning: `7bbf979d feat(stdimage)` sits in
the middle of a startup regression and is **flat against its parent**. Its name is the only thing
connecting it.

## 2026-08-25 — correcting KI-63, and the measurement method that manufactured a threshold

KI-63 went out with "0.3.11 −5.8% (no tax), 0.12.0 +27.9%". The direction holds; the single
numbers do not, and the method that produced them is unsound in a way worth recording.

**The method that failed.** Loop time as `wall(file with the loop) − wall(the identical file
without it)`. It cancels module-load cost exactly, which is why it looked right — and it is fine
while the non-loop part is small and stable. It is not fine otherwise: chasing the mechanism I
generated files with N extra `defn`s, and at 2000 the subtraction reported the loop taking
**4 ms** (the two walls are both dominated by compiling 2000 functions, and their difference is
noise). Worse, it manufactured a clean-looking **threshold** — "+0.3% at 800 functions, +64.4%
at 2000" — which in-process timing shows is not there at all: flat 24–26 ms from 0 to 4000 extra
functions. A fabricated threshold is more dangerous than a fabricated number, because a threshold
suggests a mechanism and sends you looking for it.

**What in-process timing says**, 25 runs, unpinned, `os/now-ns` either side of the loop:

| | 0.3.11 | 0.12.0 |
|---|---|---|
| 0 modules | min 23, med 23, p90 23 | min 25, med 26, p90 29 |
| 4 modules | min 22, med 26, p90 30 | min 28, med 33, p90 50 |
| tax | −4.3% min / +13.0% med | +12.0% min / +26.9% med |

Three samples of the same comparison gave a 0.12.0 tax of +27.9%, +25.0% and +12.0% on min. So
the honest claim is the **ratio** — 0.12.0's module tax is about double 0.3.11's — plus the
observation that the p90 moved most (30 → 50 ms), i.e. the distribution degraded more than the
best case. Quoting one figure would be quoting a sample.

**What is ruled out.** Not the global table: 5000 extra integer globals cost +7.5%, against
+27% for four modules. Not closure count: 4000 extra functions cost nothing. Not shared arms,
the RUNTIME GC floor, or either inliner. So it is something a *module load* does that neither
defining globals nor defining functions does — which is where the next session should start,
with `perf` and root, since `perf_event_paranoid=4` blocks profiling here.

## 2026-08-25 — KI-63 retracted: three methods, three confident numbers, no effect

KI-63 claimed loading std modules taxes JIT'd hot loops. It does not. The refutation is one
line of experiment: run the loop once and throw it away, then time a second run in the same
process.

| | 20M loop, after a discarded warm-up |
|---|---|
| no module | min 23, med 24, max 25 ms |
| `format` loaded first | min 23, med 24, max 26 ms |

Identical. Three runs in one process makes it obvious — with `format` 50 / 24 / 24 ms, without
`format` 51 / 24 / 24 ms. **The first run is slow either way.** Everything I measured was JIT
tiering, not steady-state execution.

Worth writing down how three separate methods each produced a confident wrong answer:

1. **Differencing two programs** (`wall(with loop) − wall(without)`). Cancels module-load cost
   exactly — and collapses when the non-loop part is large: at 2000 `defn`s it reported the loop
   taking 4 ms, and manufactured a clean "+0.3% at 800 functions, +64.4% at 2000" threshold that
   does not exist.
2. **Pinning.** `taskset` charged the loop for background JIT compilation, reading +68% where
   unpinned read +28% — the trap CLAUDE.md documents for `make ab`, which applies to any
   hand-rolled measurement.
3. **First-run in-process timing.** Looked rigorous — `os/now-ns` either side of the loop, 25
   runs, min and median, unpinned. Still wrong, because it timed a cold arm. And it is not even
   stable against program shape: the identical loop read a median of 25 ms as the only statement
   in a file and 40–51 ms as the first of three call sites. That shape sensitivity is what
   produced "`format` costs +92%" — `format` was never the variable.

The pattern in all three: each method was *sound in principle* and violated an assumption I had
not checked — that the non-loop part is small, that one core is representative, that the first
run is the steady state. Each produced tight spreads and plausible mechanisms, which is exactly
what makes them convincing. Tight spreads measure repeatability, not validity.

**What is real, and is the useful residue:** a whole-process benchmark of a short row measures
tiering as much as it measures the code under test. The harness does one discarded warm-up run
per *language*, which warms the boot cache — but each row runs in a fresh process, so the
program's own functions tier from cold every measured run. For rows in the tens of milliseconds
that is a large, variable share of the published number, and it is not subtracted by
`compute = wall − startup` because the `startup` row has no hot function to tier.

## 2026-08-25 — the stdimage boot-install premise, re-measured rather than inherited

KI-61's fix is to stop loading `string` and `seq` from source at boot. The mechanism for that
already exists — the stdlib image — and the reason it is not installed at boot was recorded
months ago as "the suite fails 131 of 4873". Two relevant things landed since (the `%std-edges`
edge replay, and the root-global attribution fix), and a third was found today (KI-62: the image
poisoned `require` outright on a lean build), so the figure was worth re-taking rather than
building against.

**Re-measured: 150 failures of 4920, and the recorded diagnosis is right.** Every failure is
registration-shaped — `port? is true for a fn port`, `a port record prints as itself`, `every
sealed member satisfies`, `a backend prints as`, `responses are records with a Response impl`.
Materialising a module really does define its bindings and skip every registration its load
would have performed.

Worth recording that an earlier probe of mine pointed the other way and was wrong: requiring
`queue` after an explicit `stdimage/install` showed `Seqable`, `Display`, `Conjable` and
`Inspect` all satisfied, identical to a source load. That looked like evidence the losses had
been fixed. It was not — one module surviving says nothing about the 28 that register, and I
should have run the suite before drawing a conclusion from a single probe.

**The prize is now measured, not estimated:** installing the image before the two boot requires
takes a debug-build startup from **286.9 → 180.1 ms, −37%**.

**A second blocker, solved:** `stdimage/install` cannot run that early — it reaches `os/getenv`,
`path/join` and `file/exists?` through `os`/`path`/`file`, none of which are loaded yet, so it
dies with `unbound symbol: os/getenv`. A prelude-only twin using `%getenv`, `str` and the
`file/exists?` *native* (all bound at that point) boots cleanly. That half works today; only the
150 registration failures make it unshippable.

So the remaining work is the replay, now sized: 56 `impl`, 24 `defability` and 35 `defrecord`
forms across std. Record the **forms**, not the values — symbols and lists image cleanly, where
a value snapshot cannot round-trip the closures an `impl` holds — and evaluate them on
materialise, exactly as `%std-edges` already replays require-edges.

## 2026-08-25 — why every probe said the image was fine while the suite said 150 failures

A note for whoever writes the stdimage replay, because it cost several rounds here.

Post-boot probes of materialisation do not work, and they fail in the direction that looks like
success. The shape is: install the image, `require` a module, check its registrations. Three
attempts — `queue`, `io`, `datetime` — all reported every `satisfies?` green, against a suite
that reports 150 failures with the image installed at boot.

The reason is auto-require. A qualified name resolves its module **at compile time**, so every
module the probe file mentions is loaded from source before the probe's first line runs. The
probe then inspects a source-loaded module and finds it perfect. `io` is worse still: the prelude
itself loads it, so it can never materialise in any post-boot test.

Routing through `eval` does not rescue it — the `eval`'d forms are read at runtime, but the
enclosing file still names the module somewhere, and that is enough. Nor does installing the
image earlier in the file.

So: the only measurement that means anything for this work is the full suite with the image
installed at boot. That costs a build and a suite run, and there is no cheap proxy. Budget for it
rather than trying to shortcut it, and treat a green probe as evidence of nothing.

The corollary is a small design observation: the same auto-require behaviour is why the prelude
must force-load `string` and `seq` explicitly (KI-61) — boot's namespace-resolve is a no-op for
the root prelude, so *there* the mechanism does not fire, while everywhere else it fires so
eagerly that it defeats measurement.
### Auditing the standard library's surface — and what "document everything" actually costs

A sweep over all 1707 public names, asking three things of each: can it be namespaced, is it
documented with an example, and does its name repeat its namespace. Two of the three turned out
to be mostly answered already, in ways worth writing down.

**Namespacing.** The 66 bare `*…*` names look like module config that leaked into core. They are
not: `defdyn` marks a name **ambient — never namespaced**, by design, because that is what makes
`binding` and cross-module rebinding work. Its own docstring says so. So they are bare
*correctly*, and the reflexive "move it into its module" would have broken the dynamic-variable
mechanism. The genuinely movable ones were the eight hand-named stutters (`docs/generate-docs` →
`docs/generate`, `telemetry/start-telemetry` → `telemetry/start`, …).

The stutters that remain are `defrecord` accessors — `<record>-<field>` on a record named after
its module gives `datetime/datetime-year`, and `datetime/year` already exists as a hand-written
wrapper beside it. That is duplicate surface, but changing `defrecord`'s accessor convention is a
language change with real collision risk, so it is left.

**Documentation.** The audit's first headline was "248 names have no docstring", which is true
and misleading. Of those 248, exactly **three** are `defn`/`defmacro` — and all three
(`->string`, `inspect`, `io/emit`) are ability ops. The other 245 are `def` constants, `defdyn`
variables, `defrecord` constructors and accessors: four forms with **nowhere to put a
docstring**. Every public `defn`/`defmacro` in the library is already documented.

So "document everything" is not a writing task, it is a language question: `def`, `defdyn`,
`defrecord` and `defability` ops would each need a doc slot, plus a registry — `doc` reads the
docstring off a closure, and a constant's value carries none. Left as a decision, since it
changes `def`'s arity.

**The actionable gap was examples**, and it needed a mechanism more than it needed prose.
`tests/doc_examples_test.blsp` extracts every indented `form → result` line from every public
docstring, evaluates it, and compares. Of the first 27 examples written for this pass, **seven
were wrong** — four map results written in insertion order when a map prints in trie order, and
`(reverse "abc")`, which does not merely print differently, it raises. Two more slipped in later
and were caught the same way. A wrong example is worse than none: it reads as authoritative and
nothing else in the tree evaluates prose.

`scripts/stdlib-audit.blsp` is the standing report, and both it and the example harness are
Brood — the library being audited is Brood, so the audit is a program in it rather than a script
parsing it from outside.

## 2026-08-26 — KI-61: the boot cost the namespacing waves left behind

Two changes, one measurement. Warm prelude boot **22.8 -> 11.6 ms**, base RSS **55.6 -> 50.7 MB**,
`startup` **-28.9%** (`scripts/ab-bench.sh -b HEAD -n 9 --floor`), and every other row ~11-13 ms
faster in absolute wall time — `fib` -10.5%, `loop` -10.2%, `reduce` -22.2%, `strings` -34.7%. That
last list is the point: the cost was on every invocation of `brood`/`nest`/`brood-lsp`, so the win
is not a row, it is the runtime.

**Where the time actually was.** `BROOD_BOOT_TRACE` reported one number for the cache-hit path — a
total — so the first thing was to break it into `parse`/`eval`/`freeze` (kept; the trace could not
previously say whether a boot regression was in reading the cache, evaluating the prelude, or one
`require` inside it). It read: builtins 0.8, raw positioned read of the prelude 3.5, cached-expansion
read 3.9, eval 15.7 (**of which 12.1 was two `require-one` forms**), freeze 1.9. So 15.6 of 26 ms was
two things, and neither was the interpreter.

**KI-61 (12.1 ms), fixed the other way round.** The entry's recorded fix was to make a module load
cheap at boot — the std image — blocked on a registration replay it had sized at 56 `impl` + 24
`defability` + 35 `defrecord` forms. The cheaper fix is to not load them at boot: the prelude's
references into `string`/`seq` become **autoload stubs** that load their module on first call
(ADR-246). A bare boot now loads *no* features at all. It also ends the per-wave tax structurally —
the next wave costs one declaration.

The declaration carries the arity, which is drift-prone, so both halves are guarded. The
`prelude_hygiene` lint that used to allow any reference into an `ALLOWED_MODULES` allowlist now
demands the name be a kernel primitive, a prelude definition, or an autoload declaration; a second
test asserts each declared arity still matches its module. **That test earned itself on its first
run**: `string/->number` is a *native*, so the stub I had written for it would have shadowed an
always-bound primitive with one that loads a module and forwards to itself. The re-entry guard in
`autoload-call` would have caught it at runtime, which is exactly the wrong place.

One prelude helper had to move off the module: `impl-id-owner` (the tier-ladder lookup on every
`impl` registration, the prelude's own `Display`/`Inspect` impls included) called `string/split` +
`string/join` to cut a keyword at its last `/`. Rewritten on `%str-last-index-of` +
`string/substring`, both natives — the same answer, no intermediate list, and no module.

**The unrecorded 3.5 ms.** The warm boot read the prelude *twice*: the cached expansions, and the raw
`PRELUDE` text positioned, purely so `note_definition` could record where each prelude `def` was
written (`M-.` into stdlib, ADR-031). It built the whole prelude AST on the heap to produce a
`Symbol -> (file, line, col)` map. Those names are now recorded at cache-*write* time, where the raw
forms are already in hand, and carried in the cache line (`line:col:name,... <form>`, header `v2`)
— ADR-247. The recording goes through `note_definition_recording`, i.e. `note_definition` with an
out-parameter, so `def_form_name` stays the single definition of what a form binds and the reader
cannot disagree with the writer. Def-sites are byte-identical, including the `%defseq map` case where
the name is only recoverable from the *expansion*. Not building that AST is also where the 4.6 MB of
RSS went.

**A measurement note.** Two intermediate readings were pure noise from a `make test` running beside
the timer (boot "31 ms", then "55 ms" on an *empty* program). On this machine nothing sub-10 ms is
measurable while the suite runs; the numbers above are all from an idle box through `ab-bench`, which
builds both sides through the same `make release-brood`.

## 2026-08-26 (second session) — KI-64: the `1114114` was a journal word

A JIT'd arm was writing block-argument spills onto its own deopt checkpoint, and a later read
of that slot returned the packed journal word `(resume_ip << 16 | depth)` as a live value.
`17 << 16 | 2` is 1114114, which is why the error never varied with the payload — the entry
had read it as "a codepoint counter leaking", one past `0x10FFFF`, which is a coincidence.

**The reproduction is where the time went, and every early inference was wrong.** The entry
suspected shared compiled code across processes, and the evidence looked strong:
`BROOD_NO_SHARED_ARMS=1` made it clean, it needed ~60 requests to appear, and 20 000 encodes
in one process had been measured clean. All three were misleading. Shrinking the payload
first (`seq-view of maps` fails, `vector of maps` is clean, `seq of ints` is clean) pointed at
the mutual recursion rather than at JSON; dropping JSON entirely gave a 30-line walker with
the identical value; and then **sequential** spawning failed *harder* than concurrent (0/40 vs
11/40), which killed the race theory. One process, no `spawn`, fails on call 4.

At that point `BROOD_NO_SHARED_ARMS=1` **still failed** — it had only been changing when the
arm got hot. The flag that actually named the machinery was `BROOD_NO_DEOPT_RESUME=1`, the
only one in the matrix that names a mechanism rather than a policy. The lesson worth keeping:
**a flag that "fixes" a bug is evidence about scheduling until the repro is minimal**, and
`--all`-style flag matrices invite exactly that mistake.

The second lesson: **decode the bad value before theorising about how it travelled.** One
`eprintln` at the failing `empty?` gave `raw=[0x2,0x110002,…]` — a well-formed `Int` whose
payload is a packed journal word, not a plausible datum. That is a one-line experiment that
would have skipped the entire shared-code detour.

**Root cause** in `jit_spill_reserve`: `if non_tail_call_count(code) < 2 { return 0 }` gated
the *whole* spill reserve, but the reserve has two halves and block-argument slots (KI-49) do
not depend on the call count — they depend on how deep the operand stack is at a block leader,
which a single non-tail call reaches as soon as an `if` sits inside its argument list. The
affected arms were ordinary: `fold-loop`, `index-of-seq-from`, `json/emit-list`, `any?`, most
`match-*` predicates. Arms without a journal wrote *past the frame top* instead —
`register-impl-check-arity` wanted 11 such slots against a 10-slot frame.

**The fix took three attempts, and the middle one is why ADR-248 exists.** Removing the bad
gate (giving every lowerable arm the slots it wants) is correct and costs **+2 to +7% on
nearly every row** — `max_leader_depth` counts int/bool merges that need no slot, so every
small predicate grows by a slot `push_frame` nil-inits and the JIT never writes. Bailing
instead is also correct and free, but withdraws native code from exactly those hot small arms.
The shipped form clamps the window to the reserve, so the ≥2-call shape KI-49 measured is
bit-identical and everything else deopts at the boundary as it did before KI-49. Flat across
`ab-bench --all`; `supervisor` read +17.2% then −13.5% on two runs with a 12–17% base-vs-base
floor, which is simply unresolvable and claimed as neither.

The backstop bail stays although the clamp makes it unreachable, and the invariant lives in
two `jit_plan` tests rather than only in the behavioural one — because the backstop alone
turns this class of slip into *silently lost JIT coverage*, which no output test can see.
## 2026-08-26 — every package's `:brood` floor was a lie

Eighteen repos, and sixteen of them declared `:brood ">= 0.5.0"`. That constraint (ADR-209) is
checked at project setup, and it exists to stop a user installing a package their runtime cannot
run. It stopped being true at **0.10.0**, and four consecutive releases have broken it since: the
namespacing waves (`path/`, `bytes/`, `seq/`, `rand/`, `os/`), the `io/write`/`io/puts`/`io/inspect`
output trio, and the `string/interp`-vs-`string/format` split. `bedit` alone uses eleven names that
did not exist at the floor it advertised.

Nothing caught it because nothing *can*: CI builds every package against brood `main`, so the only
version any of them is ever tested at is the newest one. A floor is a claim about the versions you
**don't** test.

**The correction has to be a new release, not an edit.** A published release's metadata is
immutable, so `hatch 0.4.12` will go on claiming `>= 0.5.0` forever. Raising the floor narrows the
supported runtime range, which is a minor bump: `store`/`s3`/`store-postgres` -> 0.3.0, `hatch` ->
0.5.0, `bedit` -> 0.3.0, the four themes -> 0.2.0. The themes' `:enhances` narrowed from
`bedit >= 0.1` to `>= 0.3` for a related reason — with a brood floor of 0.13.0, bedit 0.1 and 0.2
cannot host them at all, so the old range described a combination that could not exist.

Publishing had to run in dependency order (`store` before `store-postgres` and `hatch`), because
the intermediate state is genuinely broken: the moment `hatch`'s pin moved to `store ^0.3.0`, it
could not resolve at all until that version existed.

**Two things the stale pins had been hiding.** `store-postgres` pinned `store` at an exact `0.2.5`
— a release predating the renames — and three of its tests failed on it; on `^0.3.0` the suite is
44/44. `hatch-demo` pinned `hatch 0.4.9`, and once that moved, `nest check` could finally resolve
far enough to report six unfixed renames (`getenv`, `now-ns`, `random-token`, `tls-self-signed`,
`string->number`, and `!` -> the gen `cast`). A dependency too stale to resolve is a dependency
that hides every error behind it.

**Two repo-hygiene bugs found on the way.** This repo's own `project.blsp` said `:version "0.1.0"`
at Cargo's `0.13.0` — twelve releases of drift in a field nothing derives, so there is a test now.
And `nest new`'s `.gitignore` template never listed `.brood/`, the startup-image cache, so every
scaffolded project committed that binary and re-dirtied it on each check; eight repos were carrying
it. Fixed in the template, with a test beside the `_deps/` one that should have caught it.

**What is still not solved.** The floor is accurate *today* and will be wrong again the moment
0.14.0 renames something, because the constraint language takes one term — there is no way to say
"0.13.x only" without an upper bound that would block users the day a compatible 0.14 ships. A
floor that needs re-publishing across nine packages on every language release is a treadmill, not
a fix; what would actually settle it is the stdlib surface stabilising.

## 2026-08-26 — a search box that was never wired to anything

Reported as "search on hive really does not work". Every server-side check said otherwise:
the routes resolve, `/api/v1/packages?q=` and `/packages?q=` return correct results, matching
is case-insensitive and covers descriptions as well as names, 60 sequential and 40 concurrent
requests all return 200, and the form markup is a correct `GET /packages` with `name="q"`.

That was all true and all beside the point. The broken search was the **`Filter…` box on
`/reference`**, and it was not failing — it was never connected. `docsite/render` with
`:wrap? false` emits the content fragment and leaves the page chrome to the host; its
docstring says "the CSS/script are the caller's job". Only half of that hand-off was
reachable. `render-css` was public; the script was a private `filter-script`. So the host
inlined the stylesheet, had no way to ask for the JS, and shipped an input wired to nothing.

**The reason it lasted.** Nothing about the response is wrong. It is a 200 with complete,
correct HTML; the input is present in the markup, so a test asserting the box is there
passes. No status, header, or content check can see it. It fails only if a person types
into it — which is why "is search working?" got answered "yes" from a `curl` that returned
the right rows.

`docsite/render-js` is the missing counterpart. Grepping for the pattern rather than the
symptom then found the second site immediately: `/packages/:name/docs` embeds docsite the
same way, so every published package's docs page had it too.

**The framework guard.** `docsite` is framework-agnostic std, so it cannot call into hatch —
an asset-registration API would not have caught this. What catches it is a check on the
rendered page, so hatch grew `web/audit/inert-controls`: a control does something if it
submits a form, if an inline `on…=` fires, or if page script reaches it, and one with none
of the three provably cannot act. `web/page/page` runs it in dev and warns. Each rule
declines to judge what it cannot prove — a fragment, a page with an external `<script src>`,
an id-less control that script might select by tag or class — because a lint that cries wolf
is a lint people stop reading. Validated against the live site rather than fixtures alone:
one finding on the 315 KB page that shipped broken, zero across ten hive pages that work.

## 2026-08-26 — making the release train cheap enough to run often

Four hand-kept invariants, each of which had already drifted at least once, replaced with
mechanism. The common shape: something true today that a person had to keep true, with no
signal when it stopped being true.

**The `:brood` floor is derived, not declared.** A manifest naming no `:brood` published an
EMPTY constraint, and empty means *no constraint* — a claim to run on every brood ever
released, which the resolver believes. Absent now derives `>= (brood-version)`, the runtime
that just built and tested the release. That is the only version anyone has evidence for,
and it cannot go stale the way a hand-written floor does: every package here sat at
`>= 0.5.0` long after 0.10.0 broke it, and because release metadata is immutable, fixing it
cost nine re-publishes rather than nine edits.

**The playground wasm is built at deploy time.** The site's reference page is introspected
from the pinned runtime and the playground runs the wasm, so the two must be the same brood
or the site documents one language and executes another. That was enforced by a comment
asking a human to remember, beside a 7 MB binary in git rebuilt by hand — and it drifted;
this session hit it while shipping the reference fix and had to write a KNOWN DRIFT note
into the Dockerfile. hive's toolchain stage now builds `crates/playground` from the same
checkout it builds brood from, so there is no second thing to keep in step. The wasm-bindgen
version is READ from the crate rather than written again in the Dockerfile: two copies of a
version is the same class of bug one layer down.

**One command for the ecosystem.** `scripts/release-ecosystem.blsp` fetches, checks and
tests fifteen repos in dependency order — which is not optional, since the moment hatch asks
for `store ^0.3.0` it cannot resolve at all until that version exists. Publishing is opt-in
(`PUBLISH=1`) because a released version is immutable; the default is a rehearsal, and it
exits nonzero, so it works as a gate. It stops at the first failure and refuses a dirty
tree — `nest publish` packages the working directory, so a dirty one ships bytes in no
commit.

**Examples are executed, not just rendered.** The playground snippets and the nine runnable
examples on /docs are Brood source held as strings; nothing evaluated them, so
`even?` → `math/even?` left the front page's "Map / filter" button raising `unbound symbol`.
Both sets now run in hive's suite against the brood the wasm is built from. The /docs
examples are inline `(runnable "…")` arguments rather than a data list, so the test reads
the view as data and walks it — the reader owns the escaping, where a regex would
reimplement it and drift.

**A gap left open.** `brood` takes every trailing argument as a FILE, so a script cannot
accept flags (`-- --publish` tries to open a file by that name); the release script is
configured by environment instead. And hive itself is private and has no CI, so its 101
tests — including both example guards — run only when someone runs them.

## 2026-08-26 — the bare namespace was 510 names, and 203 of them were nobody's business

An audit of every public function in `std/` and the prelude — 1,374 of them across 106 files
— against six questions: right module, no duplication, documented with an example, named
consistently with its neighbours, in the spirit of the language, and complete against the
process model. A seventh was added part-way: where one bare ability op could replace a
family of per-type functions, prefer it, because that *shrinks* the surface twice over.

**The finding that mattered.** Module namespacing was never the problem — a `defn` inside a
`defmodule` leaks no bare global and cannot shadow the prelude's internals; that was tested,
not assumed. The cost sat in root-level code (a script, a test file, the REPL), which shares
one flat global table with the prelude. Every shipped function is reserved there, so
`(def merge-sort …)` in a script failed with "it ships with Brood and cannot be redefined" —
and **203 of the 510 reserved words were private helpers**: `flip-cons`, `reverse-onto`,
`with-build`, `for-fold`, `take-acc`, `pr-limit`, `path-last-slash`. Un-callable in any
meaningful sense, absent from `doc`, and still holding the word.

The fix was the convention already written down and applied to 313 kernel names: the `%`
prefix. Three waves — the 53 `match-*` pattern-compiler helpers, the remaining 142 private
ones, then 16 that were public by accident (the raw CHAMP primitives `map-get`/`map-assoc`/
`map-dissoc`/`map-count`/`map-pairs`/`map-int-add`, and expander leftovers like
`spawn-body-thunk`, `pattern-vars`, `defmodule-clause-heads`). 1,126 occurrences, 93 files.
**Bare names 510 → 298; names reserved against a script 470 → 261.**

The rename matched on Lisp token boundaries and was unit-tested on nine cases first, because
`merge-sort` must not touch `merge-sort-n`. It found one live site the `.blsp` suite
structurally cannot see: Brood source held in a Rust string literal (`introspect.rs`
evaluating `(%require-find …)`). `scripts/stale-names.sh` is the check that closes that gap.

**ADR-161 was not in effect.** `xmap`/`xfilter`/`xremove`/`xkeep` were declared *private*
while their `%`-prefixed internals were *public* — two names for one job, with the wrong one
exposed. The four pass-through wrappers are gone; the stages are the published names.
`lmap`/`lfilter`/`lkeep`/`lremove` had the same inversion while being documented as "the
public surface" and named in a kernel error hint. Both sets are public and catalogued now.

**`ops` is gone from bare.** `(defn ops …)` in `protocol.blsp` owned one of the most
collision-prone words in the language to read a registry nothing in production called —
while five files in `std/` already bind `ops` as a `let`. It is `reflect/behaviour-ops`,
where registry-reading belongs.

**`io/inspect` could never see an `Inspect` impl.** `*show*` renders a record through
`Display`, and `send-text` applied it to all three writers — so on the inspect path the
record arrived already flattened to a string and `inspect-render` only got to `pr-str` that:
a record with a custom `inspect` printed `"$5"` where `#money<$5>` was intended. The
transform belongs to the display renderer, and now lives there.

**A flake, chased rather than re-run.** `release_bundle` failed roughly one run in five with
`ETXTBSY`. Its four tests are parallel threads of one process, each writing then exec'ing its
own bundle; a fork taken while another thread still holds its binary open for writing leaves
the child holding that write fd, and Linux refuses to exec a file any process has open for
writing. Retried on that errno — 12/12 clean after, against two reproductions before.

**Left specified, not started.** `bit-*` is the next ten bare names and obviously one
namespace (Rust already registers `string/length`, so the shape exists). Scoping it turned up
the hazard worth recording: `eval/compile/ir.rs` maps the bare strings `"bit-and"`/`"bit-or"`/
`"bit-xor"` to `PrimOp::BitAnd` for JIT inlining, so a rename that misses that line does not
fail a test — it silently stops inlining bit ops in the crypto and hash hot loops. 347 call
sites, and it wants an `make ab` gate, so it gets its own pass.

Documentation stands at 100% docstring coverage (the last 13 were written; in every case the
explanation already sat two lines above as a comment) and **4.7% example coverage** — which
is the real remaining gap, since `tests/doc_examples_test.blsp` *executes* every example, so
each one written is a test gained. It caught two of this session's own mistakes.

## 2026-08-26 (third session) — the codecs, and two ways of being wrong about where time goes

`json`, `regex` and `base64` are Brood's worst rows against native libraries, and FRONTIER
named the lever as "a bytes/codepoint fast path shared by all three". Two of the three moved;
the third turned out not to have the problem.

**Split the row before optimising it.** Both codecs are two-directional and the halves are
nowhere near equal. Warm, best-of-5 in one process: `base64` decode **29.4 ms** against encode
**7.2 ms**; `json` parse **~85 ms** against encode **~36 ms**. Optimising the wrong half would
have been invisible at the row level. (The *cold* single-shot numbers are ~2x the warm ones —
16.5/40.2 for base64 — which is a different regime again; the row is cold, the micro-benchmark
warm, and only `ab-bench` adjudicates.)

**`json`: a missing primitive, not a slow loop (ADR-249).** `string/->codepoints` is a native
with **no native inverse**, so every parser that used it to take a string apart rebuilt one
with `(apply str (map int->char cs))` — a closure call and a one-character string per code
point, then an N-way concat. `%codepoints->string` is that inverse. Parse ~85 -> ~45 ms, row
**-20.8%** against a 0.7% floor.

**`base64`: stop paying `nth` per character.** The decode loop read a CHAMP map for the reverse
alphabet and indexed both vectors through `nth`, whose 2-arity arm is a closure that re-checks
`int?`, `vector?` and the length before reaching `%vector-ref` — eight per output triple.
A dense 128-entry codepoint-indexed vector plus direct `%vector-ref` took decode **29.4 -> 15.5
ms**, row **-9.5%**. The dense vector was only ~7% of that: the rest was not calling `nth`.

**`regex` has no such gap** — its hot path is a memoised DFA whose steady state is one
`table/get`, and it measured noise on every attempt. Worth stating, because "shared by all
three" implied otherwise.

### Two wrong measurements, both instructive

**A stale binary told me a story for twenty minutes.** `std/*.blsp` is `include_str!`'d, so
editing a module and re-running the *existing* binary measures nothing — and it does not fail,
it silently reports the old code. Three "results" (40.2 -> 37.5 -> 44.1 ms) were pure noise
around an unchanged build. The check that settles it in one command: append garbage to the
module and see whether the run still succeeds. It did.

**`nth` -> `%vector-ref` in json's scanner measured -6% on a micro-benchmark and +0.4% (noise)
on the row, so it was reverted.** Twelve sites converted, each a guard-correctness judgement,
for nothing — json's cost was the per-string assembly, not the character peek. The same
micro-optimisation was worth 1.8x on base64's tight decode loop. Same edit, same reasoning,
opposite outcome: which one is right depends entirely on whether the loop is where the time is.

### The follow-up from KI-64, measured and rejected (ADR-248)

Reserving block-argument slots for any arm that *can produce a handle* — `producers > 0`, the
predicate already in `jit_spill_reserve` — is the obvious repair for the arms KI-64's clamp
leaves deopting. It is still a broad regression, because that gate is far too weak: a `first`,
a `rest`, a `cons` or any non-tail call qualifies, i.e. nearly every arm. `spawn` **+9.7%**
against a 1.4% floor, `errors-deep` +7.7% against 0.0%, `fib` +6.4% against 0.9% — and fib's
own reserve is unchanged by the variant, so that is the prelude arms it calls. Reverted; only a
per-operand handle analysis would work, and the deopt it would remove is not detectable
(`reduce` and `pipeline`, the two rows most exposed, both +0.0%).

### Two things found by accident

- **`persistent-map` and `wordcount` were dead.** The 510->298 namespace wave renamed
  `map-int-add` to `%map-int-add` and both benchmark rows died at compile. `persistent-map`
  kept appearing in `ab-bench` sweeps with plausible times and plausible deltas the whole
  while: **a row that errors fast looks exactly like a row that is fast.** Fixed in
  brood-benchmarks; both now agree with the Python column.
- **Adding a primitive changed a map's print order.** A new interned name shifts symbol ids,
  which shifts keyword hashing, which shifts CHAMP trie order. `tests/doc_examples_test.blsp`
  compared documented results as *text*, so `(seq/frequencies [:a :b :a])` failed against its
  own correct docstring. It now compares as a value where the documented side reads as one —
  verified by sabotage, `{:b 1, :a 3}` still fails. Every docstring example with a map or set
  result was fragile this way.

### Not attempted: the call protocol

`bintree`'s own arms (`make`, `check-node`, `run`) all lower — every `call-mediated-boxed`
bail on that row is a prelude arm — so its 3.6x against Elixir really is the call-convention
cost *inside* generated code, the X-register redesign FRONTIER describes. Two things argued
against starting it here: `perf_event_paranoid=4` on this box blocks sampling, so the 50%-call-
plumbing profile could not be refreshed; and FRONTIER's own ruled-out list records that
memoising the VM's half delivered -4.3%/-4.9% at ceiling 1 and **parity at the default
ceiling**, i.e. interpreted-path call work is already covered by the JIT where the published
numbers live. It needs a profiler and a session that can watch it.
## 2026-08-26 — the rest of the audit: `bit/`, `decimal/`, `proc/`, and the deferred reply

Follow-on to the namespace pass earlier today, working the audit's remaining list top to
bottom. **Bare names 510 → 268.**

**Three more namespaces, on the `string/length` pattern** — kernel primitives registered
under a module name, with a `.blsp` file declaring the namespace:

- **`bit/`** (10 names): `bit-and` → `bit/and`, `float->bits` → `bit/float->`, and the rest.
  347 call sites. The hazard worth recording: `eval/compile/ir.rs` maps the bare strings
  `"bit-and"`/`"bit-or"`/`"bit-xor"` to `PrimOp::BitAnd` for JIT inlining, so a rename that
  misses that line does not fail a test — it silently stops inlining bit ops in the crypto
  and hash hot loops. `eval::compile::tests`' `resolve_prim` assertion is the gate; it
  passes. A check confirmed no OTHER PrimOp-keyed name was renamed in any wave.
- **`decimal/`** (4 names), following the conversion idiom: `decimal/->string` is a decimal
  to a string, `decimal/number->` a number to a decimal. `decimal?` stays bare — a type
  predicate belongs beside `int?`/`float?`/`ratio?`, not behind a prefix.
- **`proc/`** for process naming: `register`/`whereis` moved in beside `proc/info`, and the
  two missing halves added — **`proc/unregister`** (a name could previously only be released
  by its process dying, so a service could not hand its name over or step down) and
  **`proc/alive?`** (liveness had to be asked for by allocating a whole `proc/info` map).

Also `brood-version`/`build-id`/`stdlib-id` → `system/*`, and `char->int`/`int->char`/
`display-width` → `string/*`.

**Two renaming hazards this pass, both caught by the suite rather than by review.** A
token-boundary matcher is safe for hyphenated names — they cannot be Rust identifiers — but
`whereis` has no hyphen, so it renamed `pub(crate) fn whereis` in `dist.rs` to something
unparseable. And `decimal` and `register` are ordinary English words: a blanket rename
rewrote them inside prose and, for `register`, inside 12 Rust sites where `"decimal"` is the
*type* name. Both were handled by matching CALL POSITION only (`(name `) and hand-editing the
single registration line. Error messages that named the old function (`bits->float: …`,
`int->char: …`) were updated too — an error naming a function that no longer exists is worse
than the rename.

**`gen` can defer a reply.** A `call` clause had to produce `[reply next-state]`
synchronously, so a server could not hand work off without blocking its own loop — the one
gap that limited what could be *built* rather than what was convenient. New `defer` clause:
the body returns the next state (like a cast) and binds `reply`, an opaque token, so the
answer can be sent from anywhere once it exists. `(gen/reply token value)` delivers it.
Verified end to end: with a deferred call outstanding, the loop still served two casts and a
query, and the answer arrived from a spawned worker.

**`task` and `await` compose.** `(task/await (task/task f) ms)` was meaningless — `await`
took a *thunk* and started a fresh process. It now takes either.

**`pq` and `multimap` get `Conjable`.** Both had declined it on the reasoning that "conj's
single element can't carry a key/priority" — but the default impl already answers that with
the `[k v]` pair convention, and without an impl `(conj q 3)` failed with "adding to a RECORD
takes a [k v] pair or a map": an error about records, raised for a priority queue.

**A correction to the audit.** `supervisor/delete-child` is not missing — `terminate-child`
already drops the child from supervision. Adding it would have been duplication.

**Documented examples: 65 → ~170 executed cases**, across `path`, `queue`, `pq`, `set`,
`math`, `encoding`, `url`, `uuid`, `version`, `text`, `multimap`, `seq`, `bytes`, `stats`,
`rand`, `diff`, `fuzzy`. Four modules are now at 100%. Writing them found two documentation
bugs the prose had asserted for a long time:

- `multimap/keys` claimed "insertion order". It returns the CHAMP trie's order, which is not
  insertion order and **not even stable across processes** — it follows keyword interning, so
  the same two keys come back in either order depending on what else the process has loaded.
  The example failed in the suite and passed standalone, which is how it surfaced. Now
  documented as unspecified, with the example sorted.
- An example inserter that did not escape quotes terminated a docstring early and leaked the
  rest into the function BODY — `path/basename` compiled fine and raised `unbound symbol:
  a/b/c.txt` when called. It went unnoticed because only `doc_examples_test` was re-run after
  that batch, not the suite. The repair pass then over-corrected, escaping the docstrings'
  own closing quotes and four real code lines whose *strings* contain a `→`. Both were caught
  by the full suite, which is the argument for running it rather than the targeted subset.

Still open: ~1,150 public functions carry no example. That is a campaign, not a pass — but
each one written becomes an executed test, so it pays for itself.

## 2026-08-27 — `os` is the operating system, `system` is the runtime

`std/os.blsp` and `std/system.blsp` were not two modules for one concern, as the audit
recorded — they were one boundary drawn in the wrong place, which is why `system/env` was
literally `(os/getenv name)`: one function, two names, two modules. Redrawn:

- **`os`** is the OPERATING SYSTEM: `os/env`, `os/env-all`, `os/type`, `os/cmd`,
  `os/run-process`, `os/hostname`, `os/exe-path`, the tty predicates, the clock, the
  clipboard — and now the CHILD-PROCESS family.
- **`system`** is the Brood RUNTIME itself: `system/argv`, `system/script-args`,
  `system/halt`, `system/brood-version`, `system/build-id`, `system/stdlib-id`, and
  `system/features` / `system/feature?`.

That boundary is what decided the two open questions. `proc` held green-process
introspection AND OS children, so `proc/spawn` started an operating-system process while the
bare `spawn` started a green one; the OS half is now `os/spawn` / `os/write` / `os/close` /
`os/set-binary`, and `proc` is green processes only. **`proc/spawn` for the green one was
never available** — `spawn` is a *special form*, not a function, so it cannot be namespaced;
`os/spawn` against a bare `spawn` is the clearer pairing anyway. And `feature?`, which had
been sitting in `std/prelude/string.blsp`, asks what this runtime was built with, so it went
to `system` beside `brood-version`.

`proc/send` became **`os/write`**: it writes to the child's STDIN, and in this language
`send` means message-passing to a mailbox. The `[:proc handle data]` message tags are
deliberately unchanged — a receive pattern that stops matching does not fail, it silently
drops the message, which is not worth churning for symmetry.

**Three traps, all mine, all worth recording.**

*A dedupe turned an alias into a self-call.* `system/env`'s body was `(os/getenv name)`;
renaming `os/getenv` → `os/env` made the body call itself. It did not overflow the stack —
proper tail calls made it an infinite loop in O(1) memory, so the symptom was a HANG at the
first `(os/env …)`, with boot itself fine and every module loading fine. Bisecting by line
found it in a minute; guessing would not have.

*A rename ran over a file after moving code into it*, producing `(defn system/feature? …)`
inside module `system` — a self-qualified definition. Move first, then rename, or the rename
sees the moved text.

*`features` is an English word.* The token-boundary matcher rewrote prose ("build features")
and Rust identifiers (`pub(super) fn system/features`) alike. Reversed and redone by call
position plus the one registration line — the same treatment `register` and `decimal` needed.

**And a stale binary cost twenty minutes.** After fixing the self-call I rebuilt only
`--bin brood`; `target/release/nest` still carried the infinite recursion, so every
`nest test` hung and the corpus suites looked like a 25× regression. CLAUDE.md warns that a
stale binary "fails by agreeing with the baseline" — it also fails by disagreeing
spectacularly. Rebuild both binaries, or check the mtime before believing a timing.

## 2026-08-27 — migrating the ecosystem across the waves, and two outages

hive and its whole closure (hatch, store-postgres, store, s3) moved onto current brood. The
migration itself was mechanical — `int->char`, `char->int`, `whereis`/`register`, the `bit/*`
family, `decimal/of`, `os/env`, `os/spawn`, `map-pairs` → `%map-pairs`, `bytes/->list` → `seq`
— and every one applied with `nest rename` so the rewrites stayed CST-scoped (`decimal` never
touched `decimal?`).

What is worth recording is the two outages, because neither was a language bug and both were
the same mistake in different clothes: **verifying something other than what shipped**.

**The first.** Bumping BROOD_REF past the renames while the pinned hatch still called
`int->char`. The runtime built; the BUNDLE died at module load. `nest check` had passed —
against my locally installed brood, which was not the pinned one. Fix: build a toolchain at
the exact pin (a throwaway `git worktree`) and check every dependency against THAT.

**The second, sharper.** I committed hatch, then found `os/getenv` while checking against a
newer brood, fixed it in the working tree, and pinned hive at the commit from before the fix.
Everything I then ran — check, 969/969, 103/103 — tested my working tree, which is not what
`nest fetch` would clone. **A pin can only be verified against a pushed commit.** The recovery
step that now matters: `rm -rf _deps && nest fetch`, so the verification reads exactly what the
build will.

**And the check that would have caught both:** booting it. `nest test` never runs `main`, and
both failures were on the module-load path. Filed as KI-66 with `nest release --smoke` as the
fix; KI-67 covers the other blind spot, `nest check`'s deliberate silence inside a `try` body,
which let hatch ship a dead spool write (`bytes/append` → `file/spit-bytes-append`) with every
gate green.

**Three measurement lessons, all mine.** `strings` is absent from `debian:bookworm-slim`, so
`strings … | grep -c cranelift` reported 0 for *command not found* — indistinguishable from
"no JIT", and I built two deploys and a whole theory on it before checking whether the probe
ran. `grep -ac` works. Second: a 200 is not a rendered page — `/reference` returned 200 while
serving white panels on a dark ground. Third: cascade LAYERS outrank specificity, so every
component override written in `@layer components` was silently discarded by daisyUI's own
later layer; `.btn.btn-primary` was the wrong instrument because specificity never gets
consulted across layers.

Also this session: hive's `[:task-error ^h _msg]` discarded the handler's error message, which
is why its 500s were undiagnosable for months; hatch's access log never subscribed to
`[:hatch :request :exception]`; the playground's autocomplete offered a fraction of the library
(a bare `Interp` holds only the prelude — 986 names against 3632) while suggesting 1615 private
internals.

## 2026-08-27 (second session) — the stdlib image, and three measurements that were measuring nothing

Picked up the compile-parity thread the previous session was killed mid-way through (OOM,
exit 137, in the middle of re-running the suite with the image installed). The question it
was chasing: Brood is the only compiled benchmark column that recompiles its *libraries*
every run, and the stdlib image is the fix, but it "cannot be switched on" because
materialising a module skips its registrations.

**Nobody had measured that.** Three figures were on record — 131 of 4873, 150 of 4920, 170
of 4888 — and all three are artifacts of the same mistake, one that KI-61's own entry warns
about two paragraphs above where the numbers are quoted. Installing the image from a
*program* cannot exercise it: a qualified name auto-requires its module at compile time, so
the test framework and its dependency tree load the whole library from source before the
program's first line runs. Instrumented with a trace on the image branch, that configuration
reports **99 sections installed and materialises exactly zero modules**. I nearly published
the mirror-image error in the other direction: my first run of the same shape came back
**4916/4917** and looked like the gap had closed on its own.

The install had to move into the **prelude** — the only place that precedes the first
`require`. `stdimage/image-path` and `stdimage/install` are now one-line calls into it, so
there is one definition rather than a boot twin drifting from the shipped one. Measured that
way: **157 failures of 4917**, in four independent faults, and the one on record was neither
the largest nor correctly described (ADR-256).

- **112 were a concurrency race**, not a registration gap. The image branch called `provide`
  before following the module's require-edges, justified by a comment reading the `defmodule`
  macro expansion. The macro is not the loader: the source path compiles the whole file
  before evaluating any of it, and every qualified reference auto-requires during that
  compile — so a source load has all its dependencies, *body* references included, before it
  provides. Providing first publishes a module whose deps are missing; a racing process
  called `uuid/v7` and died on `unbound symbol: rand/token`. **Every affected file passed
  when run alone**, which is why this had never been seen: the suite runs test files
  concurrently, and single-file reproduction — the first thing you reach for — hides it.
- **97 were the ability impls** — the recorded gap, and real.
- **The rest were ability declarations, records, multimethods and module docs**, plus
  **reserved names** (ADR-166): a std module's functions reserve as its `def`s evaluate, and
  materialising evaluates none, so `(def path/join …)` was accepted.

**The recorded design's premise was false, and it was the expensive half.** It said
registrations had to travel as FORMS because "a closure nested in a snapshotted value does
not round-trip". Ten lines of probe: it round-trips and the impl calls correctly. Forms would
have needed each defining module's namespace re-established to resolve their bodies' bare
names. Registrations travel as values, partitioned by the owner tag each registry already
records (`*impl-from*`, `*ability-owner*`, `*method-from*`, or the qualifier on a record id),
so attribution is exact rather than inferred from a probe load.

**Result: 4917/4917 with the image installed at boot**, against 4917/4917 without. `json`
loads 6.5 → 1.7 ms, `http` 12.0 → 3.6 ms, `regex` 4.7 → 1.1 ms, `datetime` 3.2 → 1.0 ms; the
`json` benchmark row −5.6% end to end.

**A win that was a regression until it was measured from both ends.** The first working
version made `startup` **20% worse** (20.4 → 24.4 ms) while making module loads faster. The
install runs at boot on every invocation, and it was eagerly loading three bookkeeping
sections — **6.8 ms of a 20 ms boot**, 2.9 ms of it merging *every* module's require-edges
into the live registry under a lock, for modules the program would never load. That is
precisely the trade `startup` exists to catch, and the module-load benchmark alone would
have called it a win. The install now reads the index and stashes coordinates (**823 µs**,
startup at parity); the tables load on the first materialise, and a run that materialises
nothing never pays.

Kept opt-in (`BROOD_STDIMAGE=1`). Flipping the default changes every program's load path,
and one green suite run is not this repo's bar for that.

**Also fixed:** `primitives_do_not_borrow_a_non_module_slash_namespace` was red on `main`
from the morning's `math/` wave — `math/floor`, `math/numerator` and `math/denominator` are
kernel primitives under a real module, so `math` joins the allowed prefixes beside `bit`,
`decimal`, `proc` and `system`.

**Two habits this session paid for, both already in CLAUDE.md.** Instrument before believing
a pass — the trace that counted materialised modules is what separated a real green from a
green that ran none of the code under test. And do not rebuild the binary while a
verification runs: I did, mid-suite, and had to discard the batch and re-run against a frozen
copy rather than argue that the change was harmless.

**Merge follow-up — one red that came in with the ability rename, not with the image.**
`a_shipped_closure_requires_its_modules_on_the_receiver` failed on both nextest tries, and
reproduced at loadavg 1 with the stall report saying *"the child is gone, so this was not a
stall"* — deterministic, not the KI-38 load shape it resembles. Confirmed against a worktree
at `origin/main` **without** this session's commit: red there too.

The cause is a real module edge, not a bug. `std/string.blsp` reaches `math/max` and
`math/->fixed`, and `std/rand.blsp` reaches `math/rem`, so since the morning's `math/` wave
anything that formats a string pulls `math` onto the boot chain. The test asserts node A has
"never heard of math" before a shipped closure arrives — a premise that quietly stopped being
true. Its own guard is what caught it (`throw "node A already had math loaded — the test
proves nothing"`), which is the argument for writing that kind of guard: without it the test
would have gone on passing while proving nothing. Re-pointed at `encoding`, which is genuinely
off the boot chain; every assertion it makes is unchanged.

Worth knowing rather than fixing: loading `math` costs microseconds, so this is not a startup
regression — but `string` is on everyone's path, and a future wave that moves something
heavier under it would be.

## 2026-08-27 — the checker looks inside a `try` (KI-67), and it found two dead call sites

`try`/`%try`/`error-of`/`assert-error` were `SpecialHead::SkipBody`: the checker returned
without descending. The reason was sound — `(error-of (first 5))` deliberately misuses
`first`, and flagging it would make the test suite unlintable — but it threw out a whole
diagnostic class with it. An **unbound symbol** inside a `try` is not the failure under
test; it is a dead call site, and it is exactly what a rename wave produces. hive shipped
`(try (bytes/append path piece) (catch e …))` after `bytes/append` became
`file/spit-bytes-append`, with `nest check` clean and the suite green.

Now those four heads route through a new `ErrorTesting` arm that descends and keeps only
the unbound diagnostic.

**Two attempts, and the first one is the lesson.** Gating each lint on a `SUPPRESS_*` bit
looked natural — the bitmask and the `(check-allow :category …)` machinery already existed.
It was wrong within one run: exhaustiveness and `*: no num-mul method` have no bit, so
`nest check` went 0 → 6 warnings, four of them false positives on tests that misuse things
on purpose. Adding a bit per lint is a treadmill, and every future lint starts out wrong.

The second attempt filters **at the collection point** — walk into a scratch `Vec`, retain
only messages starting with `UNBOUND_PREFIX`. One place, and a lint added later is
suppressed inside these forms by default, which is the correct default for a form whose
purpose is to exercise failure. `nest check` back to 0.

**What it caught immediately**, both invisible to every existing gate:

- `tests/http_test.blsp` called `bytes-concat`; the name is `bytes/concat`. Wrapped in
  `assert-error`, so the test *passed* — on the unbound error, never on the CRLF-injection
  refusal it claimed to verify. A test asserting a failure will happily accept the wrong
  failure.
- `std/tool/mcp.blsp`'s `shadows-for` called four `project-*` helpers that are `defn-`
  (module-private), inside `(catch _ nil)`. So the MCP per-file shadow report has been
  returning nil unconditionally. Fixed by adding a public `project/file-shadow-warnings`
  where the private helpers live, rather than widening four names.

Guarded by `unbound_inside_an_error_testing_form_is_still_flagged` and
`only_unbound_survives_an_error_testing_form`, sabotage-verified.

**KI-66 closed the same day** — not a missing capability. `nest run --for <d>` already exits
nonzero with the raised error if the entry point dies, so it always was the boot check; the
gap was that nothing ran it. `package-ci.yml` now takes an opt-in `boot-check` input
(default off — a library has no `:main`, `bedit`/`pong` need a GUI).

## 2026-08-27 (fourth session) — v0.14.0: three gates that had stopped gating

**Asked to look at outstanding issues.** `known-issues.md` showed no open bug and the
working tree was clean, so the tree looked green. It was not: **every completed CI run since
2026-08-26 14:12 had failed**, and everything after was `cancelled` by the next push, so the
run list showed no red — only a wall of cancellations with an `in_progress` at the top. Two
jobs were red on all three completed runs.

**KI-69 — `differential (tree-walker)`.** KI-64's two new `jit_plan` guards assert on
VM-compiled arms; the job runs `BROOD_VM=0`, so `dbg_compiled_arms()` yields nothing. Both
guards *refuse to pass vacuously* (`only {checked} lowerable chunks inspected — a green
result would mean nothing`), which is exactly right and is why they failed instead of lying.
The fix is the pin `compile/tests.rs` has carried since ADR-222 made the tier ceiling
coherent: `set_forced_ceiling(Some(Tier::Native))`. Reproduced locally under `BROOD_VM=0`
before and after.

**`examples + stress still run` — 22 harnesses, three rename waves deep.** `examples/life`
called `map-pairs` (the enumerator is `seq` now), `examples/node_server` a bare `register`
(`proc/register`). `stress/` and `scripts/fuzz/stress/` named `os/getenv`, `rem`, `quot`,
`mod`, `min`, `max`, `string-length`, `read-all`, `read-string`, `gen/spawn-server`. One
genuine opt-out: `eval_forward_ref.blsp`'s two names are defined by `eval`, and that
invisibility to the checker is the thing the harness exists to prove (KI-24), so it takes
`(check-allow :unbound …)` rather than a rewrite.

**KI-68 — and then the fuzz-differential gate turned out to be hollow.** Fixing the corpus
surfaced it: `python3 stress/fuzz_programs.py --seeds 20` printed `seed N ok (exit=1)` twenty
times and concluded `all configs agree`. The generator writes Brood from **Python** string
literals, and the waves had retired every name in it — `(table)`, `rem`, `bit-and`, the
`table-*` family, `println`, the linear-map whitelist. Every program died on `(def t (table))`
**identically in all four configs**, and a differential reads identical death as agreement.

Three gaps, each sufficient alone: the generator is `.py`, so `nest check`, the `.blsp` suite,
`make check-stress` and `stale-names.sh` all miss it (the same class as Rust-embedded Brood);
`run_one` captured **stdout only**, discarding the `unbound symbol` on stderr before the
comparison saw it; and the `ok` line prints the compared value's last line, which for a dead
program is the literal string `exit=1` — on screen twenty times a run, reading as success.

The names are fixed (60 seeds, 0 unbound, real digests, all configs agree), but the durable
half is the assertion: **an `unbound symbol` in a generated program is now a hard failure**
naming the dead names, and under it a run where *not one seed* reached a clean exit fails as
`the corpus is dead, not the engines agreeing`. Sabotage-verified in the original shape —
reverting `(table/new)` to `(table)` gives `DEAD PROGRAM seed=1 … : table` where it used to
give `ok`. The two arms are complementary: a dead name in an *untaken branch* still exits 0,
and `check_soundness` catches that one instead.

> **The reusable lesson.** A differential gate proves N engines *agree*; it proves nothing
> about whether they agreed on anything. Any harness whose pass condition is "the sides
> match" needs a separate assertion that the sides did real work — the shape of KI-39's
> silent annotate step and KI-62's image that installed nothing. When the corpus is
> *generated* rather than checked in, that assertion belongs in the generator.

**Doc drift, in the two docs that are actually load-bearing.** `docs/language.md` and
`docs/brood-for-claude.md` still taught `print`/`println`/`eprint`/`eprintln` (retired when
the `io/` trio landed in 0.13.0) across 24 sites, `spawn-server` (now `gen/start`), and an
arithmetic reference asserting `quot`/`mod`/`rem`/`floor`/`min`/`max` are bare when they
moved to `math`. Every replacement was probed against the running binary first, and the
`gen` example re-run end to end. `brood-for-claude.md` is baked into the binary and dropped
into every scaffolded project, so a dead name there propagates to every assistant that reads
it — which is also why the next step is `nest update-tooling` in each downstream repo.

Care in the other direction too: the previous session fixed four *prose corruptions* where a
rename turned an English word into a function name, so ordinary uses of "print", "printing"
and "a print depth" were left alone here.

**Cut as v0.14.0.** No language change — the release is the tree going green.

## 2026-08-27 (fifth session) — KI-70: the checker's silent coverage boundary

**Found by accident, chasing something else.** While updating `../hive`'s dropped
`brood-for-claude.md`, `bin/deploy.blsp` and `src/web/views/docs.blsp` turned out to call
bare `min`/`max` — retired to `math` in the ADR-227 wave. Except `nest check` in hive said
nothing, and a probe file with the same call in the same shape said `unbound symbol: max`.

The difference was not the name, the module, the deps or the check cache — all four were
ruled out one at a time (a fresh broken file at `src/` **and** at `src/web/views/` was
caught; appending a broken `defn` to `docs.blsp` itself was caught; `BROOD_NO_CHECK_CACHE=1`
changed nothing). What was left was the *shape*: the live call sits inside
`[:textarea {:rows (str (max 2 …))} code]`.

**One line.** `check_into_inner` opened with `let Value::Pair(_) = form else { return }`, so
a vector or map literal ended the walk. Everything inside `[…]`/`{…}`, arbitrarily deep, was
unchecked by every lint. That is the entire Hiccup style — hive's web layer, `std/editor/*`,
every UI spec in the language.

Minimal repro, five shapes, one warning where there should be five:

```lisp
(defn a (x) (zzz x))                       ;; flagged
(defn b (x) [:tag (zzz x)])                ;; silent
(defn c (x) {:k (zzz x)})                  ;; silent
(defn d (x) [:tag {:k (zzz x)}])           ;; silent
(defn e (x) [:tag {:k (str (zzz x))}])     ;; silent — the hive shape
```

**The fix descends into vectors and maps (keys as well as values), and it is safe for three
reasons that all had to hold** before turning it on under a CI gate that rejects any
warning: the checker runs on **macroexpanded** forms, so a `match` pattern vector is already
`let`/`if` binders; `quote`/`quasiquote`/`comment` return at `SpecialHead::SkipBody`, reached
at the enclosing pair's head, so quoted data never arrives; and the generic operand recursion
is already gated on `head_is_macro`. Argued, then *confirmed* — the gate
`nest check std/**/*.blsp tests/**/*.blsp`, at zero since 2026-07-31, returned exactly one
warning, and it was real.

**That one warning was the fifth dead `project-*` call site.** KI-67's sweep found four in
`std/tool/mcp.blsp` two commits ago; the fifth, `project-all-files` in the `callers` tool,
sat inside `{:references …}` where the walk never went. So the MCP `callers` tool has been
raising `unbound symbol` on every invocation. Now `project/all-files`, public and
de-stuttered beside `project/source-files` — the remedy KI-67 already established.

> **The reusable lesson.** A lint that is *suppressed* somewhere leaves a trace you can grep
> for. A lint that is never *reached* leaves nothing. Every "return early if this is not the
> shape I expect" line in a checker is a silent coverage boundary, and the far side of it is
> exactly where nobody has been looking. KI-67 and KI-70 are the same bug at two depths;
> the second was invisible to the sweep that fixed the first.

**Also caught:** v0.14.0 was pushed with three `.blsp` files failing `nest format --check` —
the one gate in `bin/ci`'s sequence I had not run locally before pushing. Cut as v0.14.1.

**Two follow-ups the same day, both about the record rather than the runtime.**

`doc_refs::every_ki_reference_resolves_to_a_known_issue` went red: KI-69 had an index row and
no `## KI-69` section. Written. And the session collided with a parallel one — **two
different issues were both numbered KI-70** within minutes (the checker's literal-walk gap,
and a note on reversed-args renames), because both sessions read the index rather than the
headings. Renumbered the newer to KI-71; the older is cited in the CHANGELOG, the devlog, a
hive test and the **v0.14.1 tag message**, which cannot be corrected — which is the whole
argument for why a duplicate is worse than a dangling reference.

Nothing caught it, and the reason is instructive: `doc_refs::defined()` collects headings
into a `BTreeSet`, so two entries sharing a number collapse to one and every citation still
"resolves". A third gate that could not fail, on the same day as three others. Added
`no_two_entries_claim_the_same_number` (sabotage-verified: `KI-70 has 2 sections`).

`known-issues.md`'s header was a paragraph of 2026-07-era trivia about KI-9/10/17-25, all of
which the index already says. Replaced with the thing a contributor actually needs: how to
take a number without colliding, the five questions an entry must answer (**"why it
survived"** being the one most often skipped and most valuable — it is where the next bug is
hiding), the requirement to sabotage-verify a guard *and record the red output*, what each
status means, which document a finding belongs in, and how to tell the tree is green.

`handoff.md` was replaced too — it had opened "As of 2026-08-19 … `main` is green on all five
CI jobs" for eight days, which was false for the last two, in the one document whose job is
to be read cold.
## 2026-08-27 (third session) — a 79% deopt rate that is not a cost, and the build that faked it

Went looking for the next perf lever with the attribution build. `perf/measure` on a
JSON-encode workload reported **438 001 type-deopts against 552 030 native runs — a 79%
deopt rate** — and `BROOD_DEOPT_TRACE` put 790 704 of them on one arm, `json/emit`, every one
at `resume_ip=0 depth=0`: bailing at native entry having executed nothing.

That looks exactly like the nbody/`advance-body` pathology deopt feedback exists to stop, and
`json/emit` is not covered by it: `deopt_watch` is `chunk has a non-tail Call AND no
SelfCall`, and `json/emit` is recursive, so it is never watched and never self-heals. The
hypothesis was that the `SelfCall` exclusion — written for a self-tail LOOP arm, which deopts
once at the end of many productive native iterations — is too broad for a recursive tree walk
that deopts before doing any work at all.

**Measured, it is a 15% REGRESSION, and the exclusion is right.** Dropping it takes the deopt
count from 438 001 to **1** and makes the workload *slower*: warm rounds 224 → 258 ms, three
interleaved pairs, ~1–2% base-vs-base floor. The reading is that `json/emit` is genuinely
polymorphic, and the ~21% of activations that complete natively are worth more than the entry
+ deopt + VM-rerun the other 79% pay. **A deopt rate is not a cost; it is a ratio.** The
opportunity here is not to stop entering native, it is to make more of those entries succeed
(type-specialised variants), which is a much larger piece of work.

**The methodology trap, and it invalidated the first version of this result.** `make
perf-brood` and `make release-brood` write to the **same path**, `$(RELEASE_DIR)/brood` — so
after profiling, `target/release-fast/brood` is the COUNTER-ARMED binary, and timing it
against an `ab` worktree's clean build charges the working tree ~10% for atomics that are not
in the change. It showed up as two "identical" binaries (working tree reverted to HEAD)
measuring 958 vs 1018 ms. CLAUDE.md already says to confirm a winner counter-free; what it
does not say, and now should, is that the counter build *replaces* the clean one at the same
path, so the mistake needs no command-line slip to make.

Rebuilt with `make release-brood` on both sides, the control came back clean (identical
builds, ~222 ms each) and the experiment's verdict was unchanged — but nothing before that
rebuild was evidence.

**Also worth stating: cross-process best-of hides tiering.** Timing whole `brood` invocations
measures the cold-tier path repeatedly and never reaches the warm steady state. Timing rounds
*inside* one process shows both — round 1 at ~300 ms, warm at ~224 — and it is the warm
number that moved here. The two-steady-state rule in CLAUDE.md is about call count within a
run; a `for i in $(seq 9)` loop over the binary does not satisfy it.

## 2026-08-27 (fourth session) — the caches were 806 MB, and tidying them cost a whole warm boot

Went looking for the easiest real perf item and found it in `~/.cache/brood`: **4192
expanded-prelude files, 732 MB**, plus **35 stdlib images, 74 MB**. 806 MB of cache on a
machine whose whole working set is a few hundred MB.

**Both leak for the same reason, and it is not "nobody wrote a prune".** Both were bounded by
**age** — and age bounds nothing here, because both names are content/build keyed:
`prelude-expanded-<hash of build-id>.blsp` hashes the binary's mtime, so **every rebuild mints
a new ~190 KB file**, and `std-image-<stdlib-id>.bin` hashes every baked-in `.blsp`, so **every
edit to the standard library mints a new ~2 MB one**. The machines that produce these files are
exactly the machines rebuilding and editing all day, so nothing is ever seven days old. Not one
of the 4192 files was stale by the rule meant to remove it.

**The measurable cost is the prune itself.** `boot_cache_prune` walks the directory and stats
every entry on each cache-*writing* boot — i.e. the first run after every rebuild, which on a
dev machine is constant. Over 4192 entries that is **7.6 ms**, against a warm boot of **7.6 ms**:
the tidying cost as much as the thing it was tidying for. Now **0.14 ms** (54×), and the
directory is **52 MB**, 37 MB of which is `check/`'s incremental results — a separate cache,
left alone deliberately.

**Fix: bound by COUNT, keeping the newest.** 16 prelude caches (~3 MB), 4 stdlib images
(~8 MB); the age rule stays as a floor for a directory under the cap but full of dead builds.
Deleting a *recent* file another live binary is still hitting is safe by construction — that
binary pays one source boot and rewrites its own — so the failure mode is a slower boot once,
never a wrong one. The image prune is Brood (`stdimage/prune!`), the boot-cache prune is Rust,
each beside the code that writes the file.

Warm boot is unchanged at 7.6 ms and is not claimed otherwise: the prune does not run on a
cache hit, and a 4192-entry directory did not measurably slow the single `open`. The win is the
source-boot path and 754 MB of disk.

**Gated, and sabotage-verified in both directions** — the day's standing lesson is that a prune
with no test is how this rotted in the first place. Two Rust cases (count cap bounds the
directory; the age floor still fires under the cap) and one Brood case. Reverting to age-only
leaves all 40 seeded files and fails; reversing the sort so it keeps the *oldest* 16 also fails,
which is the assertion that matters — a prune that keeps the wrong 16 costs a source boot on
every binary in use, the exact cost it exists to avoid.

## 2026-08-27 (sixth session) — closing the three ungated classes

The previous entry ended by naming three places a rename could land with no gate watching.
All three are closed, and each turned out to be cheaper than expected once the actual
mechanism was found rather than assumed.

**1. The corpora were gated by RUNNING them.** `make check-examples`, `make check-stress` and
`make breakagetests` all execute the programs and fail on `unbound symbol`, which only ever
sees a name on a path a given run takes — and these files are full of branches a run does not
take. With all three green, a static `nest check` over the same trees found **74**
unresolvable names: 31 `rem`, 11 `quot`, `min`/`max`/`floor`/`->fixed`, `whereis`,
`read-string`, and the whole `table-*` family in `stress/check_corpus/`. `make check-corpora`
now runs that pass, in CI, ahead of the slow run-based gates. Nine of the 74 were genuinely
dynamic — globals created by `eval` or `system/reload-defs` in `chaos_eval_wormhole.blsp` and
`chaos2_hot_reload.blsp` — and now say so with `(check-allow :unbound …)` rather than leaving
the gate to guess. Sabotage-verified with the shape that matters: a dead name behind
`(if false …)`, which no run would ever reach.

**2. Embedded Brood, narrowed to where it actually mattered.** Two of the three surfaces were
already covered: `scaffold_quality.rs` runs `nest check` on every template `nest new` emits,
and the Python fuzz generator got its liveness assertion with KI-68. The uncovered one was the
**docs' code blocks** — 123 of them across `language.md` and `brood-for-claude.md`, evaluated
by nothing, which is exactly how both came to teach `println` and `spawn-server`.

A doc block is not a program: most are fragments over placeholders, so `doc_examples_test`'s
approach (evaluate it, compare the result) does not transfer. Two rules are decidable, and
measuring beat guessing at every step:

- Checking *every* call head is unusable — 338 distinct heads, and the unresolved list is
  `a`, `acc`, `bob`, `circle`, prose my regex caught (`ADR-146`, `Clojure-style`).
- Qualified names over the raw markdown is also unusable — file paths (`docs/types.md`) and
  prose slashes (`cast/call`, `conj/disj`) swamp it.
- Qualified call heads **inside code blocks** is clean: 24 of them, all real.

And a subtlety that would have made the gate lie: 9 of those 24 initially read as unresolved.
They are fine — a qualified reference auto-requires at *compile* time, and nothing here
compiles the block, so `bound?` must be preceded by loading the module. A gate reporting
`node/start` as missing would have been worse than no gate.

So: every qualified call head resolves, and no retired name appears — the second with an
anti-vacuity assertion that each retired name is *genuinely unbound*, so the list cannot rot
into checking for names that have since come back. It immediately found `table-put`,
`table-get` and `(table)` still in the reference, which the by-hand pass two sessions ago had
missed. Both arms sabotage-verified.

**3. The reversed-args class had a gate all along — one declaration away.** KI-71 was recorded
as a class with no possible gate: arity unchanged, nothing unbound, the type warning advisory.
That is half right. The checker catches a reversal **precisely, per argument**, cross-module:

    seq/remove-nth: argument 1 expects int, got (tuple 1, 2, 3)
    seq/remove-nth: argument 2 expects seqable, got 1

It was silent on `remove-nth` for one reason — the function had **no declared `sig`**. The
index/collection functions now carry one, and CI's zero-warning gate turns the reversal into a
hard failure. Argument types are precise because that is what catches the swap; the return
stays `any` because a narrow return would false-positive at every call site and is not what
this mistake gets wrong. Zero new warnings across std/ + tests/, and the correct order — plus
a call over untyped locals — stays silent.

> One trap worth recording: `std/*.blsp` is `include_str!`'d into the binary, so a new `sig`
> does nothing until a rebuild. The first run after adding them showed only `take-last`
> warning, from a curated Rust table, and read as "declared sigs don't work cross-module".
> They do. The binary was stale — the same class `make doctor` exists for.

## 2026-08-27 (seventh session) — the suite was never slow; the profile was

"Why is the test so long? it is a bit unrealistic." It was, and the answer was not in the
tests.

`brood_suite_passes` is **933 s of the `clippy + test` job's 1131 s**; the next slowest test
is 74 s. Two candidate causes were ruled out before the real one, because both would have
pointed at the tests:

- **Not the engine.** The `differential (tree-walker)` job runs the same wrapper at 1517 s
  against the VM's 933 s — **1.63x**, where the tree-walker is ~10x slower by design. If
  interpretation dominated, that ratio would be near 10. Solving for it puts ~90% of the time
  outside execution either way.
- **Not sleeps.** All 83 `(sleep …)` calls in `tests/` total 7.8 s.

**It is the build profile.** `cargo test` builds `opt-level = 0`, and this suite is an
interpreter exercising itself — precisely the workload an unoptimized build punishes most.
Same commit, same 4931 cases: **933 s** in CI against **58 s** for the identical suite on the
release binary. A **16x** tax with nothing to do with the tests.

`.config/nextest.toml` turned out to be a written record of that tax being paid instead of
fixed: the cap on this one wrapper went **300 → 600 → 1200 → 2700 s**, and its own comments
quote the release path doing the same work in 88 s. It also held three *sampling knobs* —
`BROOD_UCD_PART1_OF=16`, `BROOD_GABRIEL_NBOYER_MAX_N=1`, `BROOD_JDR_OF=4` — each cutting real
cases out of the CI wrapper, each justified by the same debug penalty (670 s / 256 s / 210 s
there against 3.6 s / 19.6 s / 2.7 s on release). So `make test` and `nest test` had quietly
come to cover different things.

`[profile.test]` and `[profile.dev]` are now `opt-level = 2` with `debug-assertions = true` —
optimized *and* tripwires armed, which is the combination CLAUDE.md already recommends for
manual runs, and the only thing this wrapper offers over the release path. Then the knobs
came out. Measured:

| | coverage | time |
|---|---|---|
| before (debug + 3 samplings) | reduced | **933 s** |
| after (opt-2, unsampled) | full | **66 s** |

Cost: a cold rebuild of the lib + suite binary is 20.8 s wall / 119 s CPU, so roughly 60-90 s
on a 2-core runner — against 875 s saved. The nextest budget drops 2700 s → 300 s, the first
time it has moved down.

**On thinning the suite, which was the other half of the question: the arithmetic says no.**
Measured per file: 4479 of the 4836 cases cost ~29 s *combined*, while 357 cases in twelve
files cost 22.9 s. Deleting from the cheap 93% saves nothing measurable; the expensive twelve
are expensive because they do work that is hard to reach any other way — real TCP sockets, a
stdlib image build, a 19,000-case Unicode corpus, and `mailbox_order_test`, which is **one
test costing 1.3 s** and is a sabotage-verified guard for a silent message-loss bug (ADR-195).

Three structural audits found nothing to cut either: **10 of 4954** test blocks have no
`is`/`refute`/`assert=`/`assert-error`, and all ten are legitimate (`check-property` raises on
failure, `rt` asserts internally, one deliberate smoke test); the 85 duplicate test *names*
are different modules testing different things (`"empty string"` in csv/encoding/hash/strings);
and repeated assertion lines are shared idioms within a single file.

> A methodology note, since it nearly produced a confident wrong answer: the first
> assertion-free scan reported **3266 of 4954**. That was a bug in the scanner — `\\b` after
> `assert=` can never match, because `=` is not a word character. The corrected scanner was
> validated against four known-answer cases *before* its output was trusted. A gate that
> cannot fail is the theme of this whole day, and writing one by accident while auditing for
> them is the obvious way to get caught by it.

## 2026-08-27 (eighth session) — the three questions the toolchain had no command for

Closed all five toolchain gaps the hive migration exposed ([ADR-257](decisions.md);
item 2 had already shipped with KI-67). The framing that made this cheap: three of them
are not "a check that should be stricter" but **a question nothing could ask**.

- **Does it boot?** `nest check` resolves names, `nest test` runs the suite, and neither
  loads `:main` — where both registry outages actually died (`int->char` raised *during*
  `require`, on no test's path; `os/getenv` on `main`'s first line). `nest run
  --check-boot` loads every source module and resolves the entry, running nothing; `nest
  release --smoke` then does it to the **binary just written**. That distinction is the
  point: a bundle carries a *snapshot* of every dependency, so a dep updated on disk since
  the last `nest fetch` is invisible to any source-tree check and fatal in the artifact.
  All four entry paths (`run`, `run-bundle`, both checks) now share one entry resolver —
  a check whose value is failing where the real boot fails must not have its own copy.
- **What is this binary?** `myapp --brood-build-info`. Every fact was already in the
  runtime (`system/brood-version` / `build-id` / `features`); nothing could ask the
  *artifact*. Rather than break the bundle's "argv belongs to the app" contract, the
  **`--brood-` prefix is reserved** — two names, first position only, everything else
  passed through. It reads the manifest and module directory and loads no module, so it
  answers on a broken bundle, which is when it is asked.
- **What moved, and where to?** `nest check --fix-renames` (+ `--dry-run`) runs the manual
  recovery loop. What makes it safe is what it refuses, each with its reason printed:
  ambiguity, a `%`-withdrawn target (named, never applied), and — the revert-causing one —
  **a name the project itself defines**, since `nest rename` is not scope-aware and
  renaming `register` in hive also renamed hive's own handler into the reserved
  `proc/register`. Rewrites go through the CST and `:refs-only`, so no `defn` head moves
  and a docstring or comment naming the identifier comes back byte-identical.

Plus `docsite/render-css`, the second incomplete hand-off in that API after `render-js`:
the palette is now custom properties on `.docsite` and **no rule names a colour** — which
is the actual fix, since dark values existing somewhere is worth nothing if a rule can
out-vote the host's redefinition. `render-css-dark` carries no media query, because
`prefers-color-scheme` is the viewer's OS and an embedded fragment must follow the page it
sits in — conflating those is what painted dark grey onto hive's white page. The guide
headings' hard `#1f2933`, which the old dark block never overrode (near-black on
near-black), fell out as a fix.

Every gate is sabotage-verified in both directions — a load-time unbound name, a missing
entry fn, an entry that raises if called, a healthy-project negative control — per the
previous session's lesson. Two bugs found while writing them, both worth naming because
both are the class the tooling now guards: `string/join` takes its separator **first**
(I wrote it reversed — exactly KI-71's shape), and `index-of`/`last-index-of` answer
**-1**, not nil, so a `(nil? …)` guard sails through into an out-of-bounds substring.

> Also corrected `handoff.md`, which still listed the previous session's three ungated
> classes as open: `5aa49463` closed all three (`make check-corpora` is in CI), but the
> handoff was written before that commit landed and nothing re-read it. A stale "open"
> list is the mirror image of a stale green — same cost, opposite sign.

## 2026-08-27 (ninth session) — the image does not become standard, and the three faults that says

Asked to clean the disk and make the stdlib image standard for nest projects and for the
benchmarks.

**Disk first: 91 GB.** `target/` held 100 GB, 85 GB of it `target/debug` — 73 GB of accumulated
test binaries in `deps/` (cargo never GCs those) and 12 GB of `incremental/` — plus 6.7 GB of
`target/ab` worktrees. Dropped both; the machine went from 87% full to 46%. Nothing precious:
all of it regenerates.

**The image was made the default and reverted the same day — KI-72.** `make test` caught it on
the first run, as a **120 s timeout** rather than a failure, which is the shape a stall takes.
Reduced to a clean repro: 12 parallel copies of `autoload_race --test-threads=4` against a 90 s
cap give **12 of 12 over with the image, 0 of 12 without**, while the same test passes 12/12 in
0.5 s alone. Not a deadlock and not a fault in the image — the amplifier of one. ADR-256's edge
replay must precede `provide` (or a racing process sees a module whose deps are missing, the
112-failure bug), so a module stays unprovided while it requires its whole edge set, and every
process that wants it meanwhile enters `%require-await`'s 5 ms x 1000 poll. The image puts many
more modules through that window at once, so the stalls compound. Fixing the poll — wait on the
loader finishing, not sleep-and-recheck — is the work; the flip waits on it.

Worth stating because it nearly went the other way: **six green suite runs with
`BROOD_STDIMAGE=1` and a clean `make green-all` did not catch this.** They were effectively
sequential; the stall only compounds under the full parallel suite. And a second measurement kept
the conclusion honest — at 12x4 parallelism the default arm *also* shows 4 of 12 over the cap, so
the image amplifies something pre-existing rather than creating it, and a fix has to move both
arms.

**Everything else ships**, with the duties split:

- the **runtime installs** an image when a current one exists (~0.9 ms, ~30 µs to find none) and
  **never builds** one — a build is ~1 s and `brood app.blsp` is exactly the short-lived run
  that cost would land on;
- **`nest` writes** it when missing (under `BROOD_STDIMAGE=1`), because a project tool can afford
  a second once per stdlib change. Measured end to end on a fresh `XDG_CACHE_HOME`: `brood` alone
  finds nothing and builds nothing, the first `nest` command takes 3.2 s and leaves an image, the
  next `nest test` runs in 78 ms, and `brood` then installs 101 sections from the file `nest`
  wrote.

Two more faults the flip exposed, both fixed and both worth having found:

- **`%image-write` was not atomic.** A plain `fs::write` truncates in place, and images are now
  written by `nest`, which a test suite or build script can easily run several of at once — a
  reader indexing the file mid-rebuild sees a torn image. Now a sibling temp file plus `rename`.
- **Line coverage saw nothing of the standard library.** Coverage instruments the COMPILER
  (ADR-148); a materialised module is never compiled, so with an image installed no std module
  can appear in a readout and `nest test --cover-lines` silently reports nothing for `std/`. The
  image now stands aside under `BROOD_COVERAGE`, the same way `--cover-lines` already sets
  `BROOD_NO_JIT=1`, and for the same reason: you cannot measure what ran from code that was
  restored rather than run.

**The shared key hides a production-only failure, and it nearly shipped.** `stdlib-id` hashes
every `std/**/*.blsp` **on disk** regardless of features — deliberately, so `brood`, `nest` and
`brood-lsp` share one ~2 MB file instead of writing three. But they do not bake the same
*modules*: `nest` has the dev-tools, and a lean runtime — what actually ships — does not. So
`nest` writing the image let a lean `brood` beside it **materialise `test`**, a module that
binary was built to exclude. That is the worst shape available: works on the dev box, fails in
production where no image exists.

`%std-image-serves?` closes it — a stdlib-image section is usable only for a module this binary
bakes; a project image stays unrestricted. The first attempt filtered the index at install and
cost **+1.4 ms of a 20 ms boot**, so the check moved to materialise, where it is one comparison
paid only by a module that actually loads.

**And the sabotage check lied the first time.** Forcing the guard true left the lean build still
refusing `test` — apparently proving the guard did nothing. It proved the opposite: editing
`tools.blsp` changes `stdlib-id`, so the on-disk image had gone stale and *no image was installed
at all*. Rebuilding the image at the new id after each rebuild, the sabotage reproduces exactly —
"YES — LEAKED". A sabotage that fails to break something is not evidence until you have checked
that the mechanism was live.

**Also fixed: `nest stdimage` was doing double work on a stale premise.** It built the image and
then shelled out to the `brood` on PATH to "build its own", commented as necessary because the key
was `system/build-id` (per-executable). That key moved to `stdlib-id` precisely so they would
share — so the second build wrote the same bytes to the same path, and reported
"brood: skipped (no `brood` on PATH)" whenever it was absent, which reads as a missing image and
was nothing of the kind.

**Benchmarks: `build_brood()`**, beside `build_beam()` and for the same reason. The runtime
installs an image but never builds one, so a benchmark host that had never run `nest` would
measure the source path while a developer's machine measured the image — a split across hosts, not
a fair-comparison question. The harness now writes it in the build phase. This is not a thumb on
the scale: default-on means the image is what every user's `require` already does.

## 2026-08-27 — stdlib cleanup: sequences, collections, and the word `name`

A review pass over `std/` and the language core, then the fixes. Worth recording that **three of
the seven findings did not survive contact with the ADRs** and were withdrawn rather than
implemented: `set/set` is blessed as the constructor model (ADR-236), `multimap/get` is a
deliberate per-module vocabulary (ADR-239), and `seq/vector-ref` is the measured hot path across
720 sites (ADR-164) plus an `%autoload` entry (ADR-246). Reviewing against the decision record
before writing code is cheaper than reviewing against it afterwards.

**`name` → `->string` (ADR-258).** The full rationale is in the ADR. The short version: `name`
was an ordinary English noun holding a root binding, `defmodule` emitted it bare into every
user file, and a module defining its own `name` therefore failed its own `nest check`. It is now
the `Display` op `->string`, defined once in `core.blsp` as a Brood bootstrap and taken over by
the `defability` — no `%name` primitive, so no second name for one idea.

Three things this cost that are worth knowing next time:

- **A call-position regex is not a rename.** `(name ` missed `name` in *multi-line arglists*
  (10 functions had their PARAMETER renamed instead), in `let` bindings and `defrecord` fields
  (21 more), and as a higher-order *value* (`(map name …)`, 13). Each class failed differently
  and none failed at build. The detector that actually worked was arity: `->string` takes one
  argument, so any `(->string a b c)` is damage.
- **The gates disagree about what they can see.** The in-language suite, `std_check_test`,
  `cargo test -p brood --lib`, and `cargo test -p nest -p cli --tests` each caught a *different*
  remaining site, in that order, and three of them were Brood embedded in Rust **string
  literals**, which `nest check` and the `.blsp` suite cannot see at all. Running one gate and
  believing it is how this rename would have shipped broken.
- **Renaming moved the capture hazard instead of removing it.** One commit after `name` stopped
  being captured, `->string` started being captured — by `std/text.blsp` and `std/decimal.blsp`,
  which define their own. The `/` root escape does not fix it (the resolver is a no-op for the
  prelude), so `defmodule` now emits no conversion at all.

**Sequences.** `sort` was reaching `%split-at-acc` — a helper three lines above it in the same
file — through `seq/split-at`, which fired the `%autoload` stub and loaded the entire `seq`
module to get back to the prelude. It calls the helper directly now and `split-at` leaves the
autoload list. Four helpers stranded in the prelude when their public functions moved out
(`%distinct-step`, `%flatten-acc`, `%repeatedly-acc`, `%iterate-times`) moved to `std/seq.blsp`
beside their callers and dropped the `%` prefix the module makes redundant; `%iterate-times` was
private, dead in `std/`, yet exercised by a test and cited in `docs/deferred.md` as a supported
workaround, so it is public as `seq/iterate-times`. `%has-at-least?` became `defn-`.

`std/seq.blsp`'s own header and module docstring both listed `distinct` and `zip` among the ops
that "stay bare in the prelude". They are `seq/distinct` and `seq/zip`; `(bound? 'distinct)` is
`false`. A reader following the published docstring got an unbound symbol.

**`last-index-of` → `string/last-index-of`.** String-only (it calls `%str-last-index-of` and
defaults `before` to `(string/length s)`), so by ADR-230's own boundary rule it never belonged
bare. Its forward partner `index-of` stays bare because that one really is polymorphic over
strings, lists and vectors — the pair looked symmetric and was not. This surfaced a genuine
limitation on the way: the prelude referenced it, and `%autoload` declares exactly **one** arity
per name, which cannot express an `&optional` function. The prelude calls the `%str-last-index-of`
primitive instead, which is the rule `prelude_hygiene` already states.

**Duplication found and deliberately kept**, now annotated at both ends so neither reads as an
oversight: `string/join-parts` and `seq/interpose-acc` are byte-identical, because routing `join`
through `seq/interpose` would make a prelude-reachable op drag in a second module; and
`project/project-chunk-list` and `test/run-chunks` are byte-identical because `seq` has no
list-returning chunker that keeps the trailing partial chunk — `partition` drops it and
`chunk-every` returns vectors, which does not survive the callers' `cons` (`(cons x [1 2])` is
the improper pair `(x . [1 2])`, not a list).

**ADR-236 corrected:** it states the empty constructor is `queue/new`; it shipped as
`queue/empty`, and `pq`/`multimap` followed. The code is right — `empty` denotes a value, and
`table/new` stays `new` because a table is the one identity-mutable structure, where two calls
must give two different tables.

## 2026-08-27 (tenth session) — the type system: what the audit measured, and what fixing it took

Started as a review — "where are we against a set-theoretic, gradual type system?" — and the
honest answer needed measuring rather than reading. A probe corpus through `brood --check` on
`0.14.1` separated three layers that were in very different states, and each turned out to be
a different kind of problem.

**The lattice was close; the surface feeding it was open.** `(sig q1 (strng -> int))` — a
misspelled type name — exited 0 with no diagnostic, and so did a misspelled constructor, a sig
for a name that is never defined, and (the bad one) a sig whose arity contradicts its `defn`.
That last is not a missed check but a *suppressed* one: the file being checked is never
loaded, so `arity_of` sees nothing for its own functions and the declared sig was the only
arity source a same-file call had. `nest check` passed a program that died on its first call.
Behind it sat a plainer absence — a same-file call had **no arity check at all**, the cheapest
check in the system, missing exactly where a fresh edit is. ADR-259: the definition owns the
arity, and a declaration that cannot be read is reported.

**The KI-67/KI-70 class got its gate, and the gate immediately found the next one.** Both were
`return`-early lines in the walk behind which no lint ran; both were found by accident.
`REACH_CASES` now plants an unresolvable name in every code position of every `SPECIAL_HEAD`
entry and every container literal and asserts what the walk must do with it, with a companion
test that a *new* special form must declare its own case. It runs in both walks — and that is
not thoroughness: the gate's own first sabotage attempt **passed**, because whole-file mode
caught the planted name through an unrelated pass while the arm under test did nothing. What
it found: a `quasiquote` template was skipped whole, though its `~`/`~@` escapes are code
evaluated at expansion time (ADR-260).

**Inference was the deep one.** It credited only *unconditional* demands, so the ordinary
shape of Brood code — a body that branches on what its argument is — constrained nothing:

    (defn f (x) (if (string? x) (string/length x) (+ x 1)))
    (f :kw)   ; neither branch admits a keyword, and nothing said so

The fix is to credit a guarded use *within its guard* and union the alternatives
(`D(if) = D(test) ∩ ((G ∩ D(then)) ∪ (¬G ∩ D(else)))`), which also gives `match` (its
no-match branch throws, so its domain is ⊥), head destructuring, multi-arm functions and
`:when` clause guards their domains for free — ADR-261.

**The trap in that one is worth the entry.** A prelude closure keeps its body **as written**,
so the checker meets `(cond …)` verbatim there, not the `if` chain the file path sees. Reading
an unrecognised head as an ordinary call — every operand evaluated — made `type-matches?`
demand a `seqable` first argument, because `(first t)` sits in a clause body. 21 false
positives across `tests/`, from one missing case, and the whole-tree gate is what caught it
within a minute of the change. An unexpanded *macro* call now demands nothing at all: its
operands are syntax, not necessarily code.

**And the representation.** A union of two structured types had nowhere to live — one term
holds one refinement per slot — so `(or (tuple int) (tuple string))` widened to bare `vector`
and the tagged-union idiom was invisible to every check. A `Ty` now carries an optional tail
of alternative terms (ADR-262). The single-term case is byte-identical, including its widening
merge; what changed is that a union that *cannot* merge keeps both terms, and the five set
operations quantify over them. The reason it landed without touching a single consumer: every
refinement accessor reports only for a single-term type, so consumers see exactly what a
widened type showed them — the *relations* are what got sharper.

`(not T)` closed the last gap between what the lattice could compute and what a `sig` could
say (ADR-263), and complements stopped rendering as a twenty-two-tag dump.

**The new rule is checked against the runtime, not against itself.** The soundness oracle
gained a facet for it: define a function, *call* it with a spread of values, and whenever the
call succeeds assert the argument is inside the inferred domain — a call that runs is proof
the value was in the true domain, so a domain excluding it is precisely a false positive.
Sabotage-verified (intersect the branches instead of unioning: `(and (string? x)
(string/length x))` accepts `5`, and the broken rule inferred `string`).

It also walked into a trap on its first run, which is worth more than the test: `sig_of`'s
memo is a thread-local cleared per `check_file`, so calling it directly across fresh images
reads the previous image's answer. Four different definitions all reported the same domain,
and the "unsoundness" was the oracle's own.

**One host crash fell out of writing a test.** A deep-body test for the domain walk — 20 000
levels, built by construction, since the reader caps nesting at 256 and a macro expansion does
not — aborted the process, and not in the new code: the non-tail-recursion lint's `walk`
recursed unguarded. The 2026-07-23 host-panic pass hardened that lint's *entry point* and not
the recursion that descends a function's body. Guarded, and the test now covers the shape.

Gates: `nest check std/**/*.blsp tests/**/*.blsp` at zero throughout, `make check-corpora`
green, 572 lib unit tests green, whole-tree check still 1.7 s. Two rename leftovers from the
parallel session fell out of the new checks and were followed: the curated sig for
`last-index-of` (now `string/last-index-of`) and a dead call site in
`scripts/fuzz/stress/scale_sweep.blsp`.

## 2026-08-28 — the capture hazard behind the `name` rename, and finishing the root escape

Following the ADR-258 note that renaming `name` "moved the hazard rather than removing it".
It is a real bug with a silent wrong value, not a theoretical one:

```lisp
(defmodule inventory)
(defn get (bag k) :CAPTURED)     ; an ordinary function to want
(defrecord point (x y))
(point-x (point 1 2))            ; => :CAPTURED
```

**Measured the exposure rather than guessing at it.** A scanner written in Brood
(`reflect/read-all` over the concatenated prelude, walking each `defmacro`'s quasiquote
template) found 22 emitted bare names — but that over-reports, because a name is only
capturable if it is a plain *function* (a special form resolves in the compiler) and if the
macro is ever expanded in *user* code (`%defseq` is prelude-internal). The precise list was
five macros. Two crude regex passes preceded this and both were wrong; the reader was the tool.

**The `/name` root escape was half-implemented.** ADR-236 documents it, and it worked — in a
module. `resolve()` returns early when `compile_ns()` is `None`, so at root the rewrite never
ran and an emitted `/get` reached the evaluator literally, unbound. `macros::strip_root_escapes`
now covers root: a read-only scan that allocates nothing and a rebuilding walk that runs only
when it finds an escape. It **skips `quote`/`quasiquote` subtrees**, which is load-bearing and
not merely consistent — the prelude is itself root code, so without the skip a template's `/get`
is stripped at *definition* time and the capture returns. That happened, for one build.

`defrecord`, `for`, `defonce` and `with-err-str` are fixed and proved by probe (a module
defining `get`/`reverse`/`bound?` now gets the real value back).

**Three things that did not work, each cheaper to record than to rediscover:**

- **`defmodule` cannot use the escape at all.** It *establishes* the namespace, so its
  expansion is evaluated before there is an ns to resolve against. It emits no conversion
  instead — the callee converts.
- **`receive` cannot use it either** (KI-73). Its expansion calls `but-last` from `seq.blsp`,
  which concatenates *after* `process.blsp`, so `receive` cannot expand at prelude compile
  time; `sleep`'s `(receive …)` expands lazily at first call, where no resolve pass runs, and
  the escape arrives as the literal unbound `/nil?`. Applying it turned 35 test files red.
- **Moving `sleep` below `receive` does not rescue it** — the expansion then happens during
  boot, before `but-last` exists. Tried, reverted.

**The rule is a gate now, not a convention** — `tests/prelude_capture_test.blsp`: a static scan
of every prelude template plus three behavioural probes, pinning `receive` as the known
exception so a *new* offender fails the build. Sabotage-verified both halves.

## 2026-08-28 — a record says what a value is NOT

ADR-262 made `{ok: int} | {error: string}` *representable*. It did not make it useful: a
field read through it still answered nothing, and the reason was not the union but the
records. An **open** record declared what a value must have and said nothing about
anything else, so the `{error: string}` term could not rule out an `:ok` — the honest
answer for the union was `any`, and `any` is what it gave.

So a record is now **closed**: `(record :a int)` names every key, and one it doesn't
declare is absent, which `get` reads as `nil`. `(record &open :a int)` is the marked
permissive form, spelled like Brood's other in-list markers.

**The part worth keeping is that openness is a *type*, not a flag** — `nil` for a closed
shape's undeclared keys, `any` for an open one's. One reading (`field_ty`) then drives
everything: subtyping quantifies over `keys(a) ∪ keys(b)` and compares readings, so width
subtyping falls out of `nil ⊆ any` and the required/optional rule stops being a separate
clause; disjointness is "some key's readings are disjoint", which makes the two arms of a
tagged union provably distinct with no rule written for tagged unions; and intersection is
exact per key, which matters because that is what a guard performs. Absence stopped being
declined too — `{}` really is a subtype of `{a?: int}`.

Three shapes had to stay open, and each is a *domain* rather than a value: a `defrecord`
accessor's parameter (a real value carries `:__id__` and its siblings), an ability used as
a type, and the base a path guard refines. The constructor's *result* stays closed, which
is the shape a union's field read resolves against.

**The interesting failure was downstream.** Once `(get {:x 10} :y)` typed as exactly
`nil`, `(if-let (v …) (inc v) …)` read as handing `nil` to `inc` — nothing narrowed `v`
by truthiness. Adding that guard is easy; getting it *right* was not. The exact truthy
type is "not `nil` and not the literal `false`", which this lattice cannot state, so the
sayable type is `not nil` — sound as a necessary condition for a true test, and **not**
invertible, because a false test doesn't imply `nil`. Marked biconditional it read
`(not v)` as "v is nil" and reported live code as dead, in a telemetry test. One-sided is
the honest reading, and it is the same negative-atom gap that keeps `(not (tuple int))`
from meaning what it says.

Gates: `nest check std/**/*.blsp tests/**/*.blsp` back to zero, `nest format --check`
clean, clippy clean, 583 lib unit tests. One test flipped by design — the runtime
contract's "extra keys are allowed" is now "an extra key throws", with an `&open` case
beside it, because the contract and the checker have to agree on what a declaration means.

**Stdlib naming, from the review list.** `json/parse` → **`json/decode`** (46 call sites, its
`sig`, the module docstring and the test prose): its partner is `json/encode`, and `parse`'s
opposite is not `encode`. `std/csv.blsp` was left as `csv/parse` / `csv/emit` — that pair is
coherent on its own terms (reader/writer), and churning it would trade one convention for
another rather than fix an asymmetry.

`uuid/nil-uuid` → **`uuid/zero`**. It returns the zero-UUID *string*, so the suggested `nil`
would have been wrong; and the `-uuid` suffix was a plain ADR-236 violation — the redundant
module-name prefix that ADR dropped everywhere else.

Both renames tripped the same trap the `name` rename did, in the same order: the call-position
pass missed the `sig`, the higher-order use (`(map parse results)`), the bare references in
files that `(:use json)`, and finally the prose in test names. There is a lesson here that is
now three-for-three — **a rename is not a call-site rewrite**; the classes are call, value,
declaration (`sig`), re-export (`:use`), and prose, and each one failed a different gate.

**`csv` follows `json`.** `csv/parse`/`parse-maps`/`emit`/`emit-maps` →
`decode`/`decode-maps`/`encode`/`encode-maps`. The pair was already internally coherent
(reader/writer); this is about one convention across both codec modules. Two *internal*
calls inside `std/csv.blsp` were part of the rename and are the kind a call-site sweep
misses because they are unqualified.

**Seq consolidation: measured, and the first measurement was wrong.** Counting qualified
`seq/x` references said seven functions were entirely unused. They are not — files that
`(:use seq)` call them bare, and re-counting both shapes showed every one of the 37 in use.
Nearly deleted live API on the strength of a grep. What the corrected numbers *do* show is
that **18 of 37 have zero uses inside `std/` itself** — expected for a module ADR-227
created as helpers for downstream code, so it is a product question about surface breadth,
not a defect. The one genuine structural defect found: **`first` is a kernel builtin,
`second` is in the prelude, and `third` is in `seq`** — one trio, three homes.

**And a second capture shape (KI-73).** Chasing `mapv`'s placement turned up that
`defmulti` emits `(list 'mapv '%identity-of 'args)` — quoted-head construction, which the
quasiquote scanner skips. It was a live bug: a module defining `mapv` dispatched every
multimethod on `:CAPTURED`. Five macros were affected (`map`, `apply`, `mapv`, `current-ns`,
`sig`). `sig` could not be escaped — the checker matches it structurally by head name, so
`/sig` silently disables record signature checking — which four checker tests caught before
it shipped. Gate widened to both shapes and sabotage-verified.

**And `sig` turned out to be escapable after all — the constraint was ordering.** Recorded
an hour earlier as the one name that could not use the root escape. It could; `compile`
expands before it resolves, and `macro_head_id`'s ROOT fallback (`import_of`) did not know
about `/name`, so `/sig` was not seen as a macro head, never expanded, and never produced
the `%register-sig` the checker collects. One branch in `macro_head_id` fixes it for every
macro a template emits.

The tell was that `/or` *appeared* to work while `/sig` did not. Both are macros; the
difference is that the evaluator expands macros at runtime, so `/or` got a second chance —
and the checker never evaluates, so `sig` did not. Two macros, same escape, opposite
outcomes, for a reason that has nothing to do with either name.

**On the question this raised — reserving names, or warning:** neither, now. Reserving
`sig`/`get`/`map`/`mapv` inside modules would break ADR-166's rule that a name is yours
inside a module, which is what makes namespacing worth having; and a warning is only worth
adding for a hazard that still exists. The gate asserts zero offenders. The *inverse*
problem — no way to turn a warning off per file/function/project when `nest check` exits
nonzero on any of them — is the real gap, now on the roadmap.

## 2026-08-28 — a docstring is not a string, in four editors at once

Docstrings were coloured with the string face everywhere, so a `defn`'s prose and its
return value were the same colour on adjacent lines. The rule that separates them is
positional and has three edges nobody encodes twice by accident (`def` takes no
docstring; a function's *lone* trailing string is its return value; a `defmodule`'s doc
may be the form's last element), so it went in the kernel once — `builtins::DOC_FORMS` +
the `(doc-forms)` primitive — and everything reads it: the LSP's semantic tokens (with
the `documentation` modifier), `std/editor/highlight.blsp`'s new `:syntax/doc` face, and
the VS Code / tree-sitter / Emacs artifacts `nest grammar` generates. ADR-265 has the why,
including the one case TextMate cannot get right and the LSP corrects.

**The highlighter needed a form stack, not a flag.** `hl-spans` carried a single `after`
boolean — enough for "is this symbol a call head?", not for "is this string a doc?" It now
carries a frame per open bracket (`[open n head params?]`), which is also what makes the
lone-return-string case answerable: peek the token stream past the string for a following
form.

**`brood-mode`'s indentation was fixed the same day, and it was worth measuring rather
than eyeballing.** `indent-region` over the 336 format-clean `.blsp` files in the repo
rewrote **316** of them; it now rewrites **none**. Four causes, in order of damage: Emacs
indents a `;` comment to `comment-column` and `;;;` to column 0 (Brood's formatter puts
every comment at the code indent, whatever its semicolon count) — hence a `brood-indent-line`
that indents comments as code, plus `indent-region-function nil` and `indent-tabs-mode nil`;
`brood-indent-line` passed `syntax-ppss` where `calculate-lisp-indent` needs `lisp-ppss`
(only the latter fills element 2, so the "line follows the open paren" branch answered
without ever calling the Brood hook); the formatter renders `:keyword value` as one unit at
the *body* indent, so `:isolated (test …)` lays its body out from the keyword's column, not
the bracket's; and a comment *breaks a pair*, flipping every following `cond` clause between
test and value position — so the clause walk pairs the way the formatter does instead of
counting to parity. The per-form header table that used to drive it is gone: the formatter
puts every body line +2 from its bracket, and only the two pair shapes (map / bindings list
at +1, dropped pair value at +4) depart from that.

**More of the review list.** `csv` follows `json` onto `decode`/`encode`. The three datetime
TYPE predicates (`datetime?`, `date?`, `time-of-day?`) move bare into the prelude beside
`queue?`/`pq?`/`multimap?` — ADR-236's carve-out 2 already stated the rule and the prelude
comment already spelled out the reasoning; datetime's three had simply been missed.
`before?`/`after?`/`same?` stay in the module: comparisons, not type predicates. `seq/third`
moves bare beside `second` — `first` was a kernel builtin, `second` prelude, `third` a module
function you had to know to qualify, which is one trio in three homes.

**`num-add`/`sub`/`mul`/`div` → `num/*`,** with `std/num.blsp` declaring the namespace. A
four-name hyphen prefix is exactly what ADR-251 calls a namespace spelled by hand. Two latent
gaps fell out. `numeric.rs` maps operator → multimethod by **bare string** (`"+" => "num/add"`)
and looks it up in the global table — ADR-251's recorded rename hazard in its purest form,
since a miss fails at a user's first `(+ record record)` and nowhere earlier; now pinned by a
test. And `prelude_hygiene`'s DEFINERS list did not include `defmulti`, so `num/add` read as a
reference into an unloaded module — nothing had noticed because no prelude multimethod had ever
carried a slash.

**Two audited to "no change", which is also an answer.** `defprocess` is not an outlier —
`test/deftest` and `telemetry/defevent` are the same shape, and its expansion calls
`gen-clause`, so it is a gen form rather than a language construct. `stdimage` is already
right: 8 of 13 definitions private, and every public one has an external caller.

**Third time for the same mistake, so it is a rule now.** A bulk rename must exclude the
files that *quote* the old name deliberately. `json/parse` swept ADR-220's text and two
hypothetical package names (`fastjson/parse`, `json/parser`); `uuid/nil-uuid` and `num-div`
swept `docs/wilhelm-review.md`, rewriting the very names the review questions were asking
about. `decisions.md`, `known-issues.md`, `devlog.md` and `wilhelm-review.md` are records.

## 2026-08-28 — the checker could not see most of the standard library

Continuing the type-system backlog. Three findings, in the order they surfaced.

**`to_source` renders for people, so it has to factor the aliases.** Surveying what the
checker would declare across `std/` showed `(or decimal float int map ratio)` in every
arithmetic signature — the true domain (a record joins it through the `Num` ability) spelled
five ways. `Display` factors `number`/`seqable`/`list`; the source renderer didn't, and a
generated `(sig …)` is read by a human. Fixed.

**A module-private function had no inferred signature at all** (ADR-266). `defn-` expands to
`(do (def name (fn …)) (%mark-private 'name))`, and every pass keyed on a top-level `(def …)`
saw nothing there — so its call sites went unchecked. That is 40 of `std/json.blsp`'s 42
definitions, and the internals are exactly where an argument-order slip lives. Opening the
privacy expansion fixed it; opening *every* top-level `do` was the obvious generalisation and
broke two unrelated things, which is why the descent is narrow and gensym'd names are now
never typed. Arming it across `std/` + `tests/` produced **zero** new warnings.

**Cross-term subtyping decomposes per tag** (ADR-267). Requiring each term of the left to fit
inside a *single* term of the right rejected `int | vector<int>` ⊆
`int | vector<string> | vector<int>` — a false positive, since the term's two halves land in
different alternatives. A term is the disjoint union of its per-tag projections, so placing
each projection is sound and sharper. The same projection fixed `to_source` dropping a
refinement on a term that carried tags beside it — caught by the round-trip test within
seconds of the type entering the property corpus, which is the second time that test has paid
for itself.

**And the bulk adoption path**: `nest check --suggest-sigs` prints the `(sig …)` the checker
would write for every function lacking one, over `reflect/file-signatures`. Adopting one is
sound — an inferred domain over-approximates the real one — but it stays advice, not a patch.
Informativeness is decided on the types, not the rendered text: `(string any -> any)` contains
the text of `(any -> any)` and is worth declaring.

**Later the same day — the complement of a literal** (ADR-268). `¬:ok` widened to `any`,
so a tagged-union dispatch refined only on its true side and the equality guard was marked
`then_only` because of it. A literal refinement is now `In(A)` or `Out(A)`, which makes
`(or :ok :err) ∩ ¬:ok` come out `:err` and the guard biconditional — where the guard type
is exact, which a string literal's is not (`of_value` has no heap to read the bytes, so
`(= m "x")` proves only `m : string`). Zero new warnings across the corpus; the property
corpus carries negative atoms now, and the grammar-agreement gate pins that the runtime
matcher reads `(not :ok)` and `(and keyword (not :ok))` the same way the checker does.

**And a false positive on `assoc`** (ADR-269). Adding the record sinks closed records made
load-bearing — `assoc`/`dissoc` carry the shape, `keys`/`vals` read the declared names and
types — surfaced that the neighbouring `map<K,V>` rule carried `K`/`V` through `assoc`
unchanged. `(assoc m :extra "text")` on a `(map keyword int)` was typed `(map keyword
int)`, so reading the key back gave `nil | int` and the checker flagged correct code. The
comment said "no false-positive risk either way"; claiming a *narrower* type than reality
is exactly what manufactures one. The durable half of the fix is the gate: the soundness
oracle checks map and record refinements now, not just tags and sequence elements — a
tags-only membership check passes on any map-typed expression whatever its refinement
claims, which is why this survived an oracle that had run since the refinements landed.

**And the first curated batch of `sig` adoption.** `--suggest-sigs` reports 890 undeclared
informative signatures across `std/`, which is not a number you adopt wholesale — a
declaration is authoritative, freezes inference, and deserves a reader. The subset worth
taking first is the **KI-71 class**: a function with two or more parameters of *different*
concrete types, where a reversed call is accepted in silence. There are 22; 12 of them are
in `.blsp` files (the rest are Rust builtins, which carry curated sigs already), and those
are now declared — `string/char-at`, the six `text/*` rope operations, the three
`reflect/scan-form-*` scanners, `math/->fixed`, `bytes/at`. `(string/char-at 3 "abc")` and
`(text/insert r "text" 3)` are now warnings; the corpus stayed at zero.

**The adoption turned up two defects in the tool that offers the sigs.** `(sig
string/last-index-of (-> int))` for a three-parameter function (ADR-271): an inferred
signature is a fact about types and says nothing about shape, so one whose parameters the
checker could not type came out nullary — and pasted in, Pass 2.85 rejects it. It is
reshaped to the definition's parameter list now, filling in rather than overruling, since a
multi-clause `defn` lowers to a variadic `fn` whose form-level arity would discard what the
clause inference knows. And `(or false true)` was not `bool` (ADR-270) — a different `Ty`,
unequal and with `bool <: (or false true)` answered **false** for two identical sets.
Literal slots canonicalise now, so the second spelling cannot be produced.

**A parameter in call-head position is callable** (ADR-272). Every domain rule read a
parameter's arguments; none read the position that says most about a *callback*, the head
of a call. `(defn each-of (f xs) (f (first xs)))` typed `f` as `any`, so `(each-of [1 2 3]
println)` — the argument-order slip in the shape it most often takes — was accepted in
silence. Callable is `fn | native | keyword`, because a keyword is a function of a map here
while maps, vectors and strings are not (verified, not assumed). The oracle gained a probe
whose argument is a map, which is the only one under which a keyword succeeds and therefore
the only one that catches a domain admitting merely `fn | native`.

**`gen/defprocess` → `gen/defserver`.** It never defined a *process* — `spawn`/`send`/
`receive` are core and need no import for that. It defines a **gen server**: its clauses are
`cast`/`call`/`query`/`info` and its output speaks the gen envelope (`[:$call from ref …]`).
The old name promised the core thing and delivered the framework one, which is exactly why it
read as a `def*` that had escaped the language core. It now sits with `start`/`start-link`/
`start-named`, which start the same thing. `telemetry/defevent` and `test/deftest` keep their
names: both define what they say they define.

**`==` and `not==`.** The language already had two equalities and only one had a name:
`(= 1 1.0)` is false — strict, deliberate, three tests and two doc mentions — while
`(<= 1 1.0)`, `(>= 1 1.0)` and `(compare 1 1.0)` all say equal. So `a <= b` and `b <= a`
could both hold while `(= a b)` was false: **antisymmetry was quietly untrue**, and the
numeric notion was reachable only as `(= 0 (compare a b))`. `==` names what the order
operators already believed rather than adding a third notion, and is defined *over* `compare`
so there stays exactly one comparison engine — change `compare` and `<`/`<=`/`==` move
together. The law it restores is now a test, not just a docstring.

Checked against the roadmap's own bar for a language addition, which it clears on all three
counts: it buys a capability rather than a spelling (there was no way to say this), it costs
no English word, and it is a prelude function rather than an evaluator form.

**The four remaining type-system worries, closed.** (1) An **arrow parameter was inert**
(ADR-273): `(sig apply-it ((int -> string) -> any))` bought nothing at the only site that
could use it — `(f "not-an-int")` went unchecked and `(f 1)` had no type — because the call
path consulted only global signature sources. A variable whose own type is an arrow now
describes the call it heads, consulted ahead of every global since a local shadows one.
(2) The oracle **could not reach** `map<K, V>` or arrows at all: its corpus is closed
expressions, and neither shape can arise without an annotation. A new facet types a body
under a parameter typed through the annotation parser and checks the result against the
runtime — it catches ADR-269's `assoc` defect automatically, the one that previously needed
a hand-written test. (3) What a declaration catches was **pinned rather than changed**: a
closed record catches a wrong name, a wrong type, a missing field and an extra one, while a
union that *might* be right is deliberately silent — I had misjudged this myself, so it is
now a test rather than a belief. (4) `fn | native` renders as **`fn`** (ADR-274), which is
all the language has — `type-of` says `:fn` for both — so a warning no longer names a kind
nobody can write, and a callback parameter finally has an annotation: `(or fn keyword)`.

**The comparison review — and `==` withdrawn (KI-75).** Asked to look at `=`/`<=`/`compare`
across types in detail. The design turned out defensible and the *implementation* had two
holes, both silent-wrong:

- `(compare nan x)` was 0 for every `x` — NaN equal to everything — so one NaN turned `sort`
  into a no-op returning its input unsorted, with no error. The doc comments recorded this as
  deliberate ("a `NaN` float is `Equal`"), which is exactly why it survived: consistent, and
  consistently wrong.
- `(Int, Float)` used a lossy `as f64` cast while BigInt/Decimal/Ratio-vs-Float all went
  through an exact base-10 path *with a comment explaining why the lossy one was wrong*. So
  past 2^53 two different integers compared equal.

Fixed: NaN sorts last (Rust's `total_cmp`, Java's `Double.compare`), and `Int`/`Float` joins
the exact path. `<`/`<=`/`>` stay IEEE on purpose — `compare` promises a total order because
`sort` needs one; `<` promises IEEE because arithmetic needs that. `=` stays strict.

**The `==` added earlier the same day was withdrawn on this evidence.** Built over `compare`,
it inherited both bugs — claiming `(== nan nan)` is true and that 2^53+1 equals 2^53. The gap
it was filling was real (the order operators believed a numeric equality that had no name), but
I validated the gap without validating the foundation. Reverted; the foundation is fixed now,
so it can be reconsidered on its merits rather than on a broken `compare`.

**Also answered: multi-arg ability dispatch already exists.** It is `defmulti`/`defmethod`, not
`defability` — a vector of ids one per argument, record names mixed with built-in kind keywords,
`:default` fallback, and `:commutative`/`:antisymmetric` deriving the mirror method. `defability`
is single-dispatch by design: an ability is one type's interface, a multimethod is a relation
between types. Unions in a dispatch position are the one real gap, now a roadmap candidate.

## 2026-08-28 — `std/tempo`: an unverified module, and the three defects its own tests could not see

Integrated **`std/tempo.blsp`** — a Brood adaptation of [Tempo](https://github.com/elixir-tempo/tempo)
(Kip Cole, Apache-2.0). Full notes in [tempo.md](tempo.md); the short version is that it
takes one idea from Tempo and drops the rest. The idea: **resolution is part of the value,
and every value is the half-open span it denotes.** `2026-06` *is* `[2026-06-01, 2026-07-01)`,
so a year, a month, a day and a minute are the same type at different resolutions and one set
of operations covers all of them. That deletes the `end-of-day` helper, the last-day-of-month
special case, and the class of off-by-one boundary bug that comes with instants.

On top of that: Allen's thirteen relations from one `relation` function, an interval-set
algebra closed over `union`/`intersection`/`difference`/`gaps`, unit-implied enumeration
(`parts` of June are its days — no granularity argument to pass, none to get wrong), and an
**open** `Spanning` ability so a package teaches the module about its own type with one `impl`.
`datetime/date` and `datetime/datetime` already impl it; `datetime/time-of-day` deliberately
does not, because a wall-clock time has no place on the timeline until a date anchors it.

Layered on `std/datetime` for exactly three ideas — the Hinnant civil↔days pair, `days-in-month`,
`utc-now`. That pair is the only calendar arithmetic in the system and there should be one copy.

**The interesting part was verification.** The module arrived written but *unverified* — its
author had no toolchain, and said so, with a list of the four places they expected it to break
first. It built clean, passed `nest check` at zero warnings, and passed all 79 of its own tests
unchanged. None of the four predicted failures occurred.

Three real defects did, and an independent probe of ~35 edge cases is what found them — not the
suite, which was written by the same hand as the module and agreed with it:

1. **`->iso` and `parse` did not round-trip on negative years.** `->iso` emits `-0044`; `parse`
   split on `-` and read the sign as an empty first field. The module could not read its own
   output. The docstring warned that a negative year is not interchange-safe — a true statement
   that reads as covering this, and does not.
2. **`parse` accepted a signed field.** `tp-digits?` was `(int? (string/->number s))`, and
   `string/->number` reads a sign, so `"2026-+6"` parsed as June. A predicate named for digits
   that tested for something else.
3. **`truncate` silently no-op'd on a non-unit.** `(truncate t :fortnight)` ranks `-1`, which
   fell into the "already coarse enough" branch and returned the value unchanged. A typo'd
   keyword read as "no change needed".

All three are the same shape: **a wrong answer that does not look wrong.** None throws, none is
visible at the call site, and none is reachable from an example the author would think to write —
which is the argument for probing a contributed module adversarially rather than re-running the
tests that shipped with it and calling it verified. Fixed, with regression tests; the suite is
83 tests, and the surviving behaviour (pre-epoch spans, month shift across year zero, ±100-month
shifts, leap-year day shifts, fraction truncation, the empty-set algebra) is now pinned too.

Left alone: `datetime`'s `Temporal` ability is `:sealed [datetime date time-of-day]`, so `tempo`
ships a plain `->iso` rather than joining it. **The reason recorded in the incoming notes was
wrong, and I repeated it before checking** — "`tempo` cannot join a sealed ability from outside;
`nest check` would flag an impl for a non-member". It does not. `:sealed` is an *exhaustiveness
checklist*: the checker demands every listed id implement every required op, and that is all it
does. A non-member impl from any module checks clean and runs — including from inside
`std/tempo.blsp`, where the body's `->iso` resolves to tempo's own function rather than back to
the op, so there is no recursion either. One `->iso` op can cover all four types today with
`std/datetime` untouched; widening the sealed list buys only the *demand* that the impl exist,
and that is what would reverse the dependency and change a `datetime` test. Left for its own
commit, but the cost is a line, not a refactor.

**Cashing in the signatures, and a silent bug in `nest doc`.** Declaring 306 signatures is
only worth it if they reach a reader, and the Markdown `nest doc` emits rendered the
arglist and the docstring but not the *type* — the structured doc-model record already
carried it, the human-readable heading did not. It does now, for a declared `(sig …)` and
for the curated signature a primitive carries alike, omitted where nothing types the name.

Validating that turned up a worse problem next door: **`nest doc <module>` documented
nothing for most modules, silently.** `document-module` attributed definitions by a
`(global-names)` delta across its own `require`, so a module the tool had ALREADY loaded —
transitively, through its own bootstrap — added no names and rendered a header with no
entries. `math`, `json` and `text` were all empty; `uuid` worked only because nothing else
pulls uuid in. `document-file` had already hit this and fixed it by attributing by
namespace, with its docstring naming the exact failure mode; `document-module` never got
the same treatment. It does now, delta kept as the fallback for a bare-rooted module.
`math` documents 37 entries, `datetime` 51, `seq` 39 — and `uuid` drops from 11 to **6**,
which is a correction: the delta had been crediting it with names from the modules its own
`require` pulled in.

## 2026-08-28 (second session) — a red tree, and a gate reading the wrong binary

Picked up where the previous session stopped: `092ba281` (the three require/process defects) and
its merge `6790e1b6` were **committed and never pushed** — the session ended mid-push when every
shell command started failing silently. That turned out to be environmental, not the repo: `/tmp`
is a 16 GB tmpfs mounted `usrquota` (systemd 259, `tmpfiles.d/tmp.conf`'s `q /tmp`), the per-user
slice is ~8 GB, and 13 GB of stale Claude scratchpads — one an 8.3 GB `cargo test` **debug** tree
inside a worktree under `/tmp` — had exhausted it. Writes returned `EDQUOT` while `df` read 81%
full with 3 GB free, because the filesystem was not full; the *user* was. Worth knowing because
`df` is the natural thing to check and it exonerates the real cause.

`origin/main` itself was red in three CI jobs. Both root causes were in the type-system wave.

### An adopted signature can be *less precise* than the one it shadows, silently

`type_check_catalog::checker_catches_every_should_warn_case` failed on one case:

```lisp
(+ 1 (string/capitalize "hello"))   ; expected a warning containing "+", got none
```

`std/string.blsp` had adopted `(sig capitalize (string -> any))`. A declared sig is
**authoritative** — `annot.rs` reads it ahead of primitive / curated / inferred — so it shadowed
the curated `string/capitalize : (string -> string)` and widened the return to `any`, and there
is nothing to warn about when an argument's type is `any`. Both arms of `capitalize` provably
return a string (the curated entry's own comment says so, `sigs.rs:240`), so the declaration was
just wrong. Now `(string -> string)`.

The shape is the interesting part: **adoption can lose checking, and nothing says so.** No
warning, no failing gate — the only thing that broke was a catalog entry that happened to contain
that exact expression. Ten of `std/`'s 358 declarations shadow one of `curated_sig`'s ~35 names;
a shadowed name nobody had written an example for would have gone unchecked in silence.

So it is gated structurally now rather than by example:
`types::check::tests::no_declared_std_sig_widens_its_curated_signature` walks every `std/**/*.blsp`,
parses each `(sig …)` with the checker's own `parse_sig_decl`, qualifies it by its `defmodule`,
and asserts the declared return is a **subtype** of the curated one. It also asserts it inspected
at least 8 shadowing pairs, so a broken walk or a broken qualification fails instead of passing
vacuously — the KI-68 lesson applied at the point of writing rather than after.

**Returns only, deliberately.** A declaration that *narrows* a parameter (`math/even?` declares
`int` over a curated `number`) is a tightening: it can only produce a warning the curated sig
would have missed, and `nest check`'s zero-warning gate over `std/` + `tests/` is where that
surfaces. Widening a return is the direction that loses checking quietly, so that is the
direction asserted, and the doc comment says so rather than implying the gate covers more.

### Four clippy errors that no local run could have shown

`clippy (all features)` failed with two `chunks_exact_to_as_chunks` (`types/check/infer.rs`) and
two `manual_isolate_lowest_one` (`types/mod.rs`, `types/display.rs`). Both lints are **new in
clippy 1.98**; this machine was on 1.97, where a full `cargo clippy --all-targets --all-features
-- -D warnings` passes cleanly on the identical code. CI pins `dtolnay/rust-toolchain@stable` and
there is no `rust-toolchain.toml`, so local and CI drift apart silently and the local run's green
means only "green for whatever version you happen to have".

Applied clippy's own suggestions, then **verified rather than assumed**: `rustup update stable`
to 1.98 and re-ran CI's exact invocation, which is now clean. Both APIs (`slice::as_chunks`,
`isolate_lowest_one`) were probed on 1.97 first and are stable there too, so nothing regressed
for an older toolchain. Note the CLAUDE.md warning about `--all-features` has a companion: it is
not only the feature set that arms lints, it is the *version*.

### KI-76 — the green gate was reading a 9-commit-old binary

`make green` also reported 8 `unbound symbol` warnings — `third` in the Gabriel support files,
`defserver` in `gen_test.blsp`. Both names exist. `green.sh` gated on `target/release/nest` while
telling you to run `make release`, which builds `RELEASE_DIR=target/release-fast`: a different
binary, which no documented command refreshes. It was 9 commits behind, and `std/` is
`include_str!`'d into the binary — so it was reporting that *its own baked-in stdlib* lacked
`defserver`, the name `e38e9a0b` renamed from `defprocess`. It was reading a rename backwards.
Current binary: zero warnings.

Its staleness guard could not have caught this: it was conditioned on `std/` or `crates/` having
**uncommitted** changes, so it skipped entirely on the clean tree you have right before a push —
precisely when the gate gets consulted — and it was a `note` beside a printed verdict rather than
a failure. `make doctor` had the finding all along (*"built from 464b6c57, HEAD is 6790e1b6"*),
but nothing makes `green.sh` consult it.

Fixed by resolving the binary by **identity, not path**: prefer whichever candidate reports HEAD's
sha from `nest --version` (doctor's own mechanism), and make a stale-or-older-than-source binary a
**failure** that skips the gates with `the .blsp gates DID NOT RUN`. That direction is the point —
a stale binary's verdict is meaningless in both directions, so it must not be possible to read a
believable green *or* a believable red off the wrong `std/`. Sabotage-verified in both states.

Three gate defects in two days (KI-68 dead corpora, KI-70 a walk that returned early, KI-76 the
wrong artifact) now share one sentence: **a gate must assert what it is gating on.** KI-68's
differential did not check its programs were alive, KI-70's walk did not check it reached the
code, KI-76's script did not check the binary was the tree's.

### KI-72 closed: the children died, and the image was publishing a module half-built

The only open bug, and the one blocking the biggest available wide-perf win — the stdlib image
cuts `json` 6.5 → 1.7 ms, `http` 12.0 → 3.6 ms, `regex` 4.7 → 1.1 ms and the `json` benchmark
row −5.6%, and it has shipped opt-in since the day it was flipped off.

Three prior sessions had characterised this as a lost message: root blocked in `receive`, mailbox
**empty**, all 12 workers idle, and (correctly) that the wait is unbounded rather than a 5 s
poll. The entry closed with the honest open question — *"a lost `send`, a child that died
silently, or a child that never got scheduled. Distinguishing those is where this stands."*

It is the second, and one flag answers it:

```
$ BROOD_STDIMAGE=1 ./autoload_race-… --exact racing_the_first_call_into_string_is_sound --nocapture
process 11 died: unbound error: unbound symbol: string/whitespace?
… 17 of 24 children, same error
```

`fan` uses `spawn`, not `spawn-link`, so a dead child is invisible to the root: it never sends,
and the root waits for 24 replies that will never total 24. The empty mailbox and the idle
workers were consistent with this all along and needed no mailbox or scheduler explanation.

**Why three investigations missed it.** The message is written from a *green process*, and
libtest captures output per thread — so `cargo test` and nextest swallow it. It shows only under
`--nocapture`. The bug was chased with gdb and in-language watchdogs (which, as the entry warns,
perturb the timing) and nobody read a hung run's full output. Same lesson as KI-64: read what the
program actually printed before reasoning about what it must be doing.

**The mechanism.** A section's entries are `define`d one at a time into the runtime's **shared**
global table, so each name is callable the instant it lands — and the writer ordered sections by
`(global-names)`, in which a public can precede a module-private it calls:

| entry | position in `string`'s 51-entry section |
|---|---|
| `string/blank?` (public) | 7 |
| `string/triml` / `triml-from` | 46 / 47 |
| `string/trimr` / `trimr-to` | 48 / 49 |
| `string/whitespace?` (**private**, called by all four) | **51** |

For 44 definitions `string/blank?` is bound while the `whitespace?` in its body is not. Loading
from source is immune for a structural reason: the file defines `whitespace?` at line 190 and
`blank?` at 192 — callee before caller. The image inherited no such guarantee.

That finally explains every stubborn feature: the image is the amplifier because only the image
installs in an order unrelated to source order; it is intermittent because a child must resolve
the public name inside the window; every in-language observer moved the window; re-ordering
`provide` did nothing because **the child never consults the require protocol at all** — the name
is already bound, so there is no stub to trigger a load and nothing to wait on; and one `Interp`
on one thread reproduces because the concurrency needed is between green processes, not runtimes.

**Fix: privates first** (`std/tool/stdimage.blsp` — policy stays in Brood, ADR-006). A public name
is never reachable before the privates it closes over. Sorting by definition site was tried first
and cannot work: `reflect/source-location` returns `nil` for a `defn-`, which is exactly the half
that matters.

**Measured, image verified live in every arm** (`[image] install: 103 sections` *plus* `[image]
string` materialisation lines — the entry's own trap is that a committed sha invalidates the image
and a silent 0/N then looks like a fix):

| repro | before | after | control (image off) |
|---|---|---|---|
| string test, 12s cap | 5/12 hang | **0/24** | 0/12 |
| seq test | — | 0/12 | — |
| whole binary, `--test-threads=4` | — | 0/10 | — |
| **original 12-parallel, 90s cap** | **12/12** | **0/12** | 0/12 |

The last row is the condition the entry set for calling the image safe to default on: the image
arm at *parity* with the no-image arm, not merely under the cap.

**Not claimed:** that this makes the image default-ON. That is a separate decision with its own
measurement, and the residual is recorded — privates-first does not order public→public, and full
source-order parity is unavailable because privates have no def-site. The complete fix is an
atomic section install, and the KI entry now carries the GC trap waiting in the obvious version of
it: buffering the built values in a Rust `Vec` leaves them unrooted while `from_message` keeps
allocating.

**The win now unblocked, measured on this machine** (best-of-7 whole invocations, `%now-ns`
around a single `require-one`, which is the right shape here because module load is exactly what
a short-lived run pays):

| module | image OFF | image ON | speedup |
|---|---|---|---|
| `http` | 12.93 ms | 5.98 ms | **2.2×** |
| `json` | 8.29 ms | 3.97 ms | **2.1×** |
| `regex` | 4.90 ms | 1.87 ms | **2.6×** |
| `datetime` | 4.02 ms | 2.75 ms | 1.5× |
| `seq` | 3.66 ms | 2.27 ms | 1.6× |
| `string` | 2 µs | 2 µs | — (already loaded: `io/puts` pulls `string` in before the timer starts, so this row measures nothing — kept as the reminder to check what your probe has already loaded) |

Absolutes run higher than the published FRONTIER figures on this box and the ratios are smaller
(FRONTIER has `json` 6.5 → 1.7 ms); read the ratios, not the absolutes. Either way this is 1.5–2.6×
on the cost every short-lived invocation pays — a `nest check`, a one-shot script, a handler on a
freshly spawned process — which is why the image is the widest lever left on startup and why the
public→public residual is worth closing rather than working around.

**Deliberately not attempted in this session:** the atomic section install. It is the complete fix
and it is kernel work with a live use-after-GC hazard in its obvious form (see the KI entry), so
it wants review rather than an unsupervised landing.

**Correction to the paragraph above, made the same session.** It said atomic install was "the
follow-up if a public→public instance ever appears". They already exist. `(global-names)` order is
**alphabetical** — that is *why* `string/blank?` (b) preceded `string/whitespace?` (w) — so the
identical window exists wherever a public calls a sibling public sorting later. A static scan over
`std/`'s 1318 module publics finds **≈257** such calls, three verified by hand:
`datetime/days-in-month` → `(leap-year? y)`, `datetime/today` → `(utc-now)`, and
`editor/ansi/ansi-clear` → `(ansi-clear-screen)`/`(ansi-home)`. None has been seen to fail because
none sits on a funnelled autoload path the way `string/blank?` does — the stub aims 24 processes at
it simultaneously — but the windows are there.

So the honest statement is: **privates-first is necessary and not sufficient.** It fixes the
reproducible hang; it does not make the image safe to default-ON. Atomic section install is the
prerequisite, and the kernel already has the rooting API for it (`Heap::root`/`read_root` plus
`roots_len`/`truncate_roots` in `core/heap/gc.rs`) — pass 1 builds and roots, pass 2 defines from
the roots, validated under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`.

Two process notes worth keeping. The first count I took was **757**, and it was wrong twice over:
`:year` matched the public `year` (the keyword colon was not excluded) and `(defn- foo` parsed as a
public named `-`, because `defn-?` matched `defn` and then captured the hyphen as the name. Both
inflations pointed the same way — toward a scarier number — and only hand-checking a single case
(`epoch-ms->` "calling" `year`, which is actually `(get ymd :year)` beside three `let`-locals named
`hour`/`minute`/`second`) exposed them. A static scan over a Lisp needs call-position matching and
a hand-verified sample before its count means anything.

## 2026-08-28 — a sealed ability over the numeric tower rejected its own members

Asked why so little in the stdlib is `:sealed` (15 abilities, 1 sealed), and the honest answer
turned up a bug. Most of them shouldn't be sealed — sealing puts the member list in the
*declaring* module, so it inverts who depends on whom, which is fatal for an interface meant to
be extended. Five of the fifteen have **no impls anywhere in std**; `JsonEncode`'s only impls
are in tests. Those are extension seams, and enumerating their members means enumerating types
that don't exist yet.

But two — `Numeric` and `Zero` in `std/math` — *are* closed sets, implemented over
`:int :float :decimal :ratio` and nothing else. By the rule they're seal candidates. **They
can't be sealed**, and that turned out not to be a design choice:

```lisp
(defability Sizey :sealed [:int :float] (sizey [self] :-> int))
(impl Sizey :int (sizey [n] n))
(impl Sizey :float (sizey [n] 0))
(sig use-it (Sizey -> int))
(defn use-it (x) (sizey x))
(use-it 42)     ; warning: argument 1 expects {__id__: :float | :int, ...}, got 42
```

Both are declared members with working impls. The program prints `42`. `nest check` exits 1.

`impl` dispatches on built-in kinds as well as records, but ADR-181's mechanism turned *every*
sealed member into a record shape `%{__id__: :m}` — so `:int` became "a record whose `__id__` is
`:int`", which no int is. Same in `protocol::sealed_op_domain` (the ADR-190 occurrence-typing
domain), independently.

**Why it survived: the two paths that look like the whole feature only need the id set.** The
exhaustiveness gate fires correctly over kinds. Rejecting a provable non-member fires correctly
over kinds. Only *passing a real member* exposes it — and since nothing in-tree is kind-sealed,
ADR-181's own false-positive audit ("confirmed across all of `std/` + `tests/`: zero argument
warnings introduced") was true and told you nothing. A latent trap for whoever tried it first.

Fixed: both sites route through one `annot::sealed_members_ty`, which splits members into
records (one open `%{__id__: (:a | :b | …)}` shape, as before) and built-in kinds (their own
lattice points via `base_ty`). Sharing the helper is load-bearing, not tidiness — the two
denotations must agree, or a value a `sig` accepts gets rejected by the op call inside it. The
discriminator is cheap and total: a record member is always ns-qualified, so an unqualified
name `base_ty` knows is a kind, and a record named `map` can't collide.

A seal may now mix the two. All members pass; the record half widens to `map` in the union
(`Ty::union` drops a differing `fields` map — sound, it only ever accepts more), so a mixed
seal trades `:__id__` precision for coverage and a purely-record seal keeps it. Error messages
improved on the way: `expects int | float` rather than `expects {__id__: :float | :int, ...}`.

Four regression tests, the zero-warning gate over `std/` + `tests/` still clean, 383 checker
tests and clippy `--all-targets --all-features` green. ADR-181 amended.

**Then the join it had been blocking, which was never blocked.** With sealing understood,
`std/tempo` now carries `(impl Temporal tempo/tempo (->iso [t] (->iso t)))` — so one
`datetime/->iso` renders a tempo, a date, a datetime or a time-of-day. `std/datetime` is
untouched, the dependency still points one way, and `(%sealed-members 'Temporal)` is unchanged.
The body's `->iso` is tempo's own function, so it delegates rather than recurses; a test pins
that the op and the plain function agree at every resolution. The only thing not gained is the
checker *demanding* the impl exist, which is the one thing widening the sealed list would buy —
at the price of `std/datetime` naming `std/tempo`.

**Not changed, but worth recording:** `Numeric` and `Zero` are now sealable — but on the rule
above they probably shouldn't be sealed either. `Zero`'s own docstring anticipates user types
("a type with no impl RAISES rather than answering"), and a money record answering `zero?` is a
feature, not a bug; sealing would make the *name-as-a-type* narrower than reality and reject
that record from a `(sig f (Zero -> bool))`. Being able to seal them was the point; sealing them
is not. And `Temporal` — the one ability that *is* sealed — is the counter-example: nobody can
enumerate the temporal types in advance, and `std/tempo` is the proof, arriving years later in
another module.

**Typed multimethods shipped (ADR-278)** — step 1 of the resequenced plan. `defmulti` takes a
trailing `:-> RET` beside the algebra keyword, `%register-multi` records it, and both halves
landed together: call sites get typed (`MultiInfo::ret_of` → `infer.rs`, where
`AbilityInfo::op_ret_of` already fed) **and** every method body is checked
(`check_one_method_return`). Shipping only the first would have been worse than shipping
neither — the checker would type every call `int` while a method returned a string.

The find while adopting it in std: **`compare-to`'s contract was stated twice in prose and
enforced nowhere.** The `Ord` ability declares `(compare-to [self other] :-> int)` and the
multimethod's own comment says `→ -1 / 0 / 1`, but the multimethod declared nothing — so a
method returning a string was caught by neither, and `sort` is built on it. It declares
`:-> int` now; no existing method violated it. `num/*` deliberately stays undeclared: its
result follows its operands, so `any` would say nothing.

Two things worth knowing next time. `MultiInfo::generics` is built from the *file's* forms, so
a prelude multimethod's call sites were untyped until `ret_of` gained a by-name fallback —
exact for a root-level `defmulti`, since the global carries the multimethod's name. And the
editor saved over `types/check/protocol.rs` mid-edit, silently dropping one script's changes
while the next script's landed; the tell was a build error naming a method that "should" have
existed. Re-read before assuming an edit stuck.

## 2026-08-28 — the review pass: a fix that half-fixed, and a fix that broke something narrower

Re-reviewed the day's two pieces adversarially rather than re-reading them. Three findings,
two of them in my own fixes.

**1. I fixed one site of a class and not its siblings.** `truncate` was made to throw on a
keyword that names no unit. `now` had the identical hole and failed *further from the
mistake*: an unguarded unit reaches `tp-restrict` at rank `-1`, which keeps no units at all,
so `(tempo/now :fortnight)` built a tempo with an empty unit map that only blew up later —
`expected number, got nil` from inside `tp-pad`, or from `datetime/dt-ymd->days` two modules
away. `finer`/`coarser` had the softer version: they returned `nil`, conflating "there is no
finer unit" (a real answer, for `:ms`) with "that is not a unit". A third pass found `unit`
doing it too, with the nil that means "`t` does not carry that unit". All five unit-taking
entry points — `finer`, `coarser`, `truncate`, `now`, `unit` — throw now, enumerated by
grepping the source rather than by recall, which is what should have happened first. The
lesson is the cheap one, learned twice: when you fix a silent-wrong input guard, sweep every
other function taking the same argument, and do it before reporting the fix.

The rest of the input surface was swept the same way and is fine by house convention: a wrong
type reaches a primitive and throws loudly (`(parse 42)` → `string/length: expected string`;
every float `shift` dies in `quot` inside `epoch-ms->`), and a non-`Spanning` argument gets
the ability error, which helpfully lists the impls that do exist. `unit` was the only entry
point returning a wrong answer rather than raising.

**2. The sealed-member fix introduced a narrower version of the bug it fixed.** Classifying a
member as a built-in kind by its *spelling* is wrong, because a record declared at ROOT
namespace registers under its bare name: `(defrecord ratio …)` outside any `defmodule` owns
the id `:ratio`, the same dispatch key the built-in ratio kind uses. So `:sealed [ratio]`
read as the kind and rejected `(ratio 1 2)` — a false positive where, before the change,
there had been none.

Worth recording *why* that is not simply "revert to record-wins": the language itself
conflates the two. With a root record named `ratio` and no kind impl at all,
`(sz 1/2)` — a genuine ratio — dispatches to the **record's** impl and returns its answer.
Both the old behaviour (always a record) and the new one (always a kind) are wrong in
opposite directions, because the id honestly denotes whichever exists. The tiebreak is the
`*record-ids*` registry: a registered record wins, `base_ty` only otherwise. It has to union
the runtime registry with this file's own `%record-register` forms from the expanded tree —
`nest check` expands but never evaluates, so a file's own `defrecord`s are invisible to the
registry at the moment it is checked. That detail cost a wrong first attempt that built
cleanly, passed every existing test, and fixed nothing.

**3. `std/tempo` had no cross-process coverage**, which the repo's protocol requires of
anything carrying values. Added: a tempo, a negative year and a three-span interval set
round-tripping through workers (so `to_message`/`from_message` and the promote/freeze path
are exercised), both `Temporal` and `Spanning` dispatching in a process that never registered
them, and a twelve-worker fan-out. Green under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` and over
five repeat runs.

Suite: tempo 85 → 94, ability 92, checker units 293. `nest format --check` clean, the
zero-warning gate over `std/` + `tests/` clean.

**Nothing pinned or declared the Rust version, and that is a bug (2026-08-28).** Merging
`origin/main` failed to build here with `error[E0658]: use of unstable library feature
'isolate_most_least_significant_one'` — `types/mod.rs` and `types/display.rs` use
`isolate_lowest_one`, stabilised in 1.98, against a local 1.95. The error names no version and
reads as "this code is wrong" rather than "your toolchain is old"; the first instinct is to
patch the code, which would have been exactly wrong.

The repo had **no `rust-toolchain.toml`, no `rust-version`, and CI on
`dtolnay/rust-toolchain@stable`** — a floating target with nothing recording what the code
actually needs. Freezing the toolchain would fight a deliberate choice (`package-ci.yml`:
"a warning for the whole ecosystem rather than a check against a frozen toolchain"), so the fix
is to declare rather than pin: `rust-version` in `[workspace.package]`, inherited by all five
crates, so cargo reports the requirement and the actual version instead of an E0658.

The number is **1.95**, not 1.98, and how that was settled is the useful part: two fixes landed
for this in the same hour from opposite directions — one removing the 1.98 dependency (both
sites now iterate the tag table, since the clippy lint that suggested `isolate_lowest_one` and
the 1.95 floor cannot both be satisfied by a bit trick), one declaring 1.98 as the minimum.
Committed together they contradicted: the code no longer needed 1.98, so declaring it would
have rejected toolchains that build fine. The declaration follows the code — the floor is the
oldest version actually exercised, never a guess.

Worth stating plainly because the instinct is to wave it away as environmental: a build that
fails on a supported toolchain with an error that misdirects the reader **is** a bug, and the
absence of an MSRV is what made it one.

**Second correction, after pulling: the whole writer account above is SUPERSEDED, and the merge
that brought the better one dropped it.** The same bug was root-caused twice in parallel that day.
The other account (ADR-279, `startup_image.rs`) found the piece this one missed: the reason a
racing process can enter a half-installed module at all is that installing the real `blank?`
**removes its ADR-246 autoload stub** — the one door that routes a caller into `require-one` and
makes it *wait*. So what must be deferred is not "privates" but **every name that already has a
binding**, i.e. the stubs. That is strictly more general, and it dissolves the ≈257 public→public
residual measured above: those windows are unreachable, because there is no stub to come through.
**Deferring is enough; atomicity is not needed** — so the "atomic section install is the
prerequisite for default-ON" conclusion above is withdrawn, as is "privates-first is necessary and
not sufficient".

The `std/tool/stdimage.blsp` privates-first change was accordingly **reverted**: with it removed
the repro is 0 of 24 with the image verified live 24/24, so it was redundant, and two
half-mechanisms for one bug is worse than one whole one.

Three things worth keeping from how this went wrong:

1. **The merge (`cba50894`) hit a conflict in `known-issues.md` and kept only the weaker account**,
   silently dropping 83 lines of the better one — including its methodology (relaxed per-process
   counters read out from a SIGTERM handler, because `ptrace_scope` forbids attaching gdb and every
   in-language observer moved the race) and a third measurement trap. The code from both sides
   merged cleanly and every gate stayed green, so **nothing failed**. A doc conflict resolved by
   "keep one side" loses findings no test can miss. Restored by hand; both accounts and the
   withdrawn claims are now in the KI entry.
2. **The amplifier switches itself off.** `BROOD_STDLIB_HASH` covers every `std/**/*.blsp`, so any
   edit — including one made by somebody else while a measurement loop is running — changes the id
   and the image arm silently becomes the no-image arm. The figures in this entry were taken with
   the image verified *once before the loop*, not per run, so they should not be leaned on. Verify
   inside the measuring command and report `image live N/N`.
3. **`make ab`'s json row was measuring a dead program.** `f3819889` renamed `json/parse` to
   `json/decode` at 07:26 and the benchmark corpus still called the old name, so the row died on an
   unbound symbol; the sweep surfaced it only as a 120 s timeout. `bench/smoke.py` in
   brood-benchmarks catches exactly this and names the symbol — it simply never runs when the
   rename lands in *brood*. Fixed; all 31 rows live. Worth a thought about wiring that gate to
   brood's rename waves, since the two repos cannot depend on each other in CI.

Also this session: **KI-77** filed (⚠️ watching) — the `loop` row is ~2-3% slower than v0.14.1 and
survives both the unpinned and the interleaved check, so it is not the pinning artifact and not
drift. Not bisected: that row's absolute numbers move ~3% between measurement *sessions* (the same
binary read 90 ms in one interleaved pair and 93 ms in the next), which is enough to swamp a 3%
per-step bisect. The entry says to use same-session interleaved pairs or not to bother.

## 2026-08-28 — reviewing the day's 45 commits: a guard wrong in both directions, and two messages that showed the plumbing

Reviewed today's 45 commits (208 files, ~15.6k insertions) by risk and by probing rather than
by reading diffs. Three things.

**1. ADR-278's `defmulti` guard was wrong in both directions at once.** `(defmulti f :-> nil)`
was rejected though `nil` is a legal return type, and `(defmulti f :commutative :->)` — a
dangling arrow — was accepted silently. One root cause: the guard tested `(nil? ret)` to mean
"no return type given", but **absent is not nil**; and because it also demanded
`(<= (count opts) 0)` it only fired when the arrow was the *first* option, so an algebra in
front of a typo let it through, discarding the declaration the author was reaching for.

The sharp part is that `%register-multi` was already written correctly — it takes the return
as `& more` with a docstring saying "absent must be distinguishable from an explicitly
declared nil" — and the macro defeated it by always passing a third argument, after which both
checker-side readers had to treat a nil as "undeclared" to compensate. Three layers, and the
careful one lost. Fixed all three; `:-> nil` is now genuinely enforced. (Landed concurrently
as `0ac9ff37`; the merge came out coherent, re-verified against the merged binary.)

**2. A record name could not be written as a type.** `(sig area (circle -> float))` warned
"unknown type `circle`" about a type the checker held in `*record-ids*` all along — and
`defrecord` already emits one in its own constructor sig. Sealed ability names have resolved
since ADR-181; a record is the more obvious case, and ADR-259 turning unknown names into
*reported* warnings made the gap something every user meets writing the obvious thing. I first
declined this as a "power feature" under ADR-011, which was wrong: it is not a knob, it is an
existing type that could not be named. See the ADR-181 amendments.

**3. And then the message showed the plumbing.** `expects {__id__: :t/circle, ...}, got
{__id__: :t/square, ...}` renders the representation twice over. It now reads `expects
t/circle, got t/square` — the spelling a `sig` takes — with refinements kept
(`t/pt{x: 7}`) and non-nominal maps untouched. Two of my own tests and one pre-existing one
asserted on the `__id__` marker as a proxy for "the identity is named"; all three now pin the
user-visible rendering instead, which is the better assertion.

**Also re-verified, because a default flipped underneath the day's testing:** ADR-281 turned
the stdlib image ON by default, so every green run before it described a configuration that is
no longer shipped. Re-swept: 207/207 `.blsp` files under the new default, and KI-72's own
repro — 12 parallel `autoload_race` copies — finishes in **2 s** against the 90 s cap it used
to hang past. `BROOD_IMAGE_TRACE=1` confirms the image path is genuinely exercised rather than
merely configured.

218/218 `.blsp` test files, 626 Rust lib tests, the zero-warning checker gate, `nest format
--check`, and clippy `--all-targets --all-features` all green.

## 2026-08-28 (later) — KI-77 closed by a boot win nobody recorded, and the KI-72 guard that stopped guarding

Two perf items, and both turned out to be about *measurement* rather than about code.

### KI-77: real when filed, gone at v0.15.0 — and the original reading was right

Filed earlier the same day: `loop` ~3% slower than v0.14.1, surviving both the unpinned check
(so not ADR-175's background-JIT-on-one-core artifact) and the interleaved check (so not thermal
drift). At `e9c54606` it reads **-4.3%** against the same v0.14.1 binary. The tempting conclusion
is that the original measurement was wrong, and it is worth not jumping to it: building the exact
tree the entry was filed against and measuring it in **one session** against HEAD gives
`dfcddc4f` **94 ms** vs HEAD **89 ms**. So `dfcddc4f` really did sit ~4 ms above v0.14.1's ~90 ms,
as filed. The entry was right and v0.15.0 moved past it.

**What moved is a fixed per-run cost, not `loop`.** `dfcddc4f` -> HEAD, same session, best-of-15:

| row | dfcddc4f | HEAD | delta | absolute |
|---|---|---|---|---|
| `startup` | 36 ms | 30 ms | **-16.7%** | -6 ms |
| `loop` | 94 ms | 88 ms | -6.4% | -6 ms |
| `sieve` | 84 ms | 78 ms | -7.1% | -6 ms |
| `fib` | 116 ms | 111 ms | -4.3% | -5 ms |
| `collatz` | 223 ms | 218 ms | -2.2% | -5 ms |

Every row gained the *same ~5-6 ms* and the percentage just tracks how cheap the row is. That is
a boot/load win — and it is the exact mirror of what filed KI-77, where almost every row read
slightly positive for the same reason in reverse. Reading a per-row *percentage* table without
looking at the absolute column is how a single fixed cost gets mistaken for N separate
regressions, in both directions.

**A 17% startup win with no commit next to it.** It arrived somewhere in `dfcddc4f..e9c54606`
and nothing in the devlog records it. The prelude changes in that range are ADR-278/281's
multimethod return types, which are not an obvious cause, and `cli_support.rs`'s addition there is
the `.brood_crash_dump` process-death hook (diagnostics, not boot). **Worth attributing before
anyone claims it.**

**And one arithmetic that must not be done.** It is tempting to subtract the `startup` row from
`loop` to isolate compute. CLAUDE.md and FRONTIER both record that as an under-subtraction trap:
`startup` is `(io/puts 0)`, which loads `io` and through it `string`, while most rows load neither.
If anyone wants to know whether a residual compute delta survives under this boot win, the
measurement is in-process with a fixed iteration count, not a difference of two whole-invocation
rows.

### The stdlib image is default-ON, and the KI-72 guard no longer guards it

The flip landed in v0.15.0 (`f114d01e`, opt out with `BROOD_NO_STDIMAGE=1`). Verified empirically
here rather than from the docs: with no flag set, a `require` of `json` reports `install: 103
sections` and materialises from the image; with `BROOD_NO_STDIMAGE=1` there is not one `[image]`
line. `(stdimage/status)` reports `:state :live, :sections 103`. Measured on a three-module
script: **96 ms -> 66 ms, a 31% saving** (the published figure is 46.5 -> 36.2 ms; this box is
slower, the ratio is larger, read the direction).

**But `autoload_race` — the guard for KI-72 — never builds or requires an image.** Nothing in
`ci.yml` builds one either. So in CI that test exercises the *source* path and asserts nothing
about the imaged race it exists to catch. It is not reliably vacuous, which is worse than being
reliably vacuous: `image_matches_source.rs` *does* build an image (ADR-280) and writes it to
`~/.cache/brood`, so whether `autoload_race` runs imaged depends on whether that test happened to
run first — and nextest gives each test its own process in no guaranteed order.

The differential does not cover the gap. It compares *state* — name, kind, privacy, declared
signature — and proves the two load paths agree once loaded. KI-72 was not a state divergence; it
was a **race during install**, where a public name became callable before its private helper was
bound. A differential over final state cannot see that, by construction.

So the guard was made self-sufficient, following the pattern `image_matches_source` already
established: build-and-install the image before the racing `Interp` exists, and **assert it is
live**, so the test cannot pass on the source path and silently report that KI-72 is still fixed.

**The guard, sabotage-verified — and the first attempt at it was vacuous.** Reverting the
deferral and re-running, per 12 standalone invocations of one case: the pre-existing arm caught it
6 of 12, the new imaged arm 4 of 12, `cargo test` running all five cases in one process **0 of 3
runs**, and `cargo nextest run` — what `make test` and CI use — **4 of 4 runs red**.

So the guard is **probabilistic**, and what makes it a gate is nextest running five cases in five
processes rather than any one case being reliable. Two things to carry: plain `cargo test` cannot
see this class of bug at all (all five cases share a process and the earlier ones warm the
allocator, interner and JIT enough to close the window — reproduce with nextest or `--exact`), and
the new arm is *weaker* than the old one for a knowable reason — building the image in a throwaway
interpreter warms the same process. That is this bug's documented hazard, that every in-language
observer moves it, showing up inside the test written to catch it.

Worth stating plainly: the first version of this test **passed with the bug reintroduced**, and
only sabotage said so. It asserted three preconditions (install returned non-nil,
`*std-image-file*` set, module not yet loaded) and every one of them held — the test was correct
about its own setup and still measured nothing, because it was run the wrong way. A precondition
assertion proves the setup, not the sensitivity.

### Three items documented before fixing them (KI-78, the unattributed boot win, the drifting install)

Recorded first and deliberately, because all three are things a green tree does not show you.

**KI-78 — CI tests the load path users do not get.** The stdlib image is default-ON since
v0.15.0, and default-ON is safe by construction: no image on disk means `install` returns nil in
~30 µs and `require` reads source. Nothing in `ci.yml` builds one, so that is what every job
does. The suite is green *imaged* (1222/1222 locally with one verified live), so this is a gap in
what CI proves rather than a bug behind it — but it is nondeterministic rather than absent, because
`image_matches_source.rs` builds an image and writes it to `~/.cache/brood`, and nextest schedules
cases in no guaranteed order. Same shape as the KI-72 guard hole one level down, found the same way.

The fix has a constraint that is easy to miss: building the image makes `autoload_race`'s
default-path arms imaged too, so source-path race coverage would silently vanish. One job has to
stay on source deliberately.

**The boot win nobody recorded.** v0.15.0 carries a ~5-6 ms *fixed per-run* saving that lands on
every benchmark row — `startup` 36 → 30 ms, `loop`/`sieve` −6 ms, `fib`/`collatz` −5 ms — and it
closed KI-77 as a side effect. Nothing in the log claims it. Before bisecting, the rule from
brood-benchmarks' CLAUDE.md applies: **sample three or four points across the range and look at
the curve.** A ramp means there is nothing to find and `git bisect` will still return a commit —
that is how a `primes` regression once got attributed to a `.blsp` *test file*.

**The installed binary drifts, and it is load-bearing for benchmarks.** `~/.local/bin/brood` sat
15+ commits behind most of the day. `make doctor` reports it; nothing enforces it. The harness runs
whatever is on `PATH`, so a published run against a stale or dev-tools build silently measures
something other than what `make ab` and `nest release` do. The lean install
(`INSTALL_FEATURES='$(RUN_FEATURES)'`) is the one that keeps those three on one build — and
contrary to an older note, `nest test` works fine on it.

### The benchmark publish, and two things it found that nobody was looking for

Published a full seven-language run (brood-benchmarks `9fa69bc`) at the harness defaults on the
lean installed build. 224/224 row-language combinations agreed on checksums; the only broken row
in the corpus was `json`, fixed earlier the same day.

**1. `collatz` read 95 → 185 ms and the runtime had not regressed.** The previous published run
was Brood 0.13.0 and predated the port's migration from bare `rem`/`quot`/`max` to qualified
`math/*` — a migration the namespacing waves forced, since the bare names no longer exist. So the
row had to change to keep running, and the change cost ~40%. Measured on one binary: qualified
205 ms, the same names bare via `(:use math)` 204 ms, the primitives directly 121 ms.

**Qualification is free; the wrapper is not.** `math/rem`/`math/quot` are Brood functions over one
primitive and `math/max`/`math/min` are variadic over `apply`. Fifteen of the 31 rows call
`math/*`, so it is a floor under half the suite — written up in
[compute-frontier.md](compute-frontier.md) §2b with the fix direction CLAUDE.md prescribes
(multi-arity dispatch in the evaluator, *not* Rust builtins — this is the worked example there).

**2. `make ab` could no longer build any baseline older than today.** `bfa98682` added the
`stdimage` cargo feature, and `ab-bench.sh` deliberately builds the baseline with *this* tree's
Makefile so both sides get identical flags — which forwarded `--features brood/stdimage` into a
worktree whose crate does not declare it. Cargo fails at dependency *resolution*, so it is not a
compile error you can read past:

```
package `cli` depends on `brood` with feature `stdimage` but `brood` does not have that feature
```

`make ab` exists to compare against older refs, so this broke the tool for precisely its purpose,
and it broke silently in the sense that nothing exercises it — no gate A/Bs against an old ref.
Fixed by dropping a feature the baseline's own `Cargo.toml` does not declare, checked with one
grep against that file rather than a hardcoded cutoff, so it cannot go stale the way a date would.

Two limits found while verifying the fix, both worth knowing before trusting a cross-version A/B:

- **A row whose program changed across a rename wave cannot be A/B'd across it.** `collatz` now
  calls `math/rem`, which does not exist on 0.13.0, so the baseline cannot run the program at all.
  This is the same shape as the `json/parse` → `json/decode` breakage: the benchmark corpus tracks
  the current API, so it is only valid on refs that share it.
- Rows that *didn't* change measure fine. `fib` reads **+12.9%** and `startup` +3.4% across
  0.13.0 → HEAD, and `fib` also moved +6.4% in the harness — two independent signals, not yet
  investigated. Taken together with the unattributed ~5-6 ms boot win, what is wanted is a
  per-commit sweep recording **absolutes for trend**, which is what brood-benchmarks' CLAUDE.md
  prescribes when the shape might be a ramp rather than a step.

### Retraction, same day: there was no v0.15.0 boot win, and I made this feature's own documented mistake

Earlier today's entries claim v0.15.0 carries a ~5-6 ms fixed per-run saving — an "unattributed
17% startup win" — and I put that in the handoff, in KI-77's resolution, and in a commit message.
**It is wrong.** `make ab BASE=dfcddc4f ROWS=startup` compares a worktree binary against the
working-tree binary, and the working-tree binary had a **current stdlib image** while the worktree
binary had none, because the image id carries the git sha and nothing had ever built one for that
ref. So the arm I read as a code improvement was *imaged vs unimaged*.

That is KI-72's trap 3, inverted — "the arm you believe is imaged is reading source" — and I made
it in the same session in which I restored the paragraph describing it. Three measurements of the
same quantity disagreed (−16.7%, −10.9%, +0.0%) and I attributed the first one instead of
distrusting all three. The rule that resolves it was already written down: **verify the image per
arm, not once per session.** `(stdimage/status)` exists precisely for that and reports `:live` /
`:stale` / `:absent` beside the id it wants and the ids on disk.

**What the trustworthy measurement says** — one session, all six binaries interleaved, all in the
same (unimaged) state, core-pinned, best-of-9:

| row | 0.13.0 | v0.14.0 | v0.14.1 | dfcddc4f | e9c54606 | HEAD |
|---|---|---|---|---|---|---|
| `startup` | **27 ms** | 35 | 34 | 36 | 36 | 36 |
| `fib` | 106 | 108 | 112 | 115 | 114 | 114 |
| `loop` | 84 | 91 | 89 | 93 | 92 | 95 |

- `startup` is **flat from v0.14.0 to HEAD** — nothing in v0.15.0 to attribute. What is there is an
  unrecorded **~30% step between 0.13.0 and v0.14.0**, partly given back later by the image flip,
  which is why the published numbers only show 18.3 → 19.4 ms. A step is bisectable; this is the
  real open question.
- `fib` and `loop` are **ramps** (+7.5%, +13%), the shape brood-benchmarks' CLAUDE.md says has
  nothing to find.

**Two traps found while settling it**, both worth knowing before any image measurement:

1. `make release-brood` rebuilds only `brood` (`-p cli`), so `target/release-fast/nest` stays at an
   older commit and `nest stdimage` writes an image keyed to *nest's* id, which `brood` cannot use.
   `brood` then reports `:stale` however many times you rebuild the image. Use `make release`.
2. The id includes the **git sha even when the baked stdlib is byte-identical** — three images on
   disk here shared content hash `f81c5e8bfacc125` and differed only by sha. So **any** commit
   invalidates every image, not just a `std/` edit, which is broader than trap 3 states. The sha is
   a deliberate conservative proxy for "the kernel interpreting this stdlib may have changed", so
   it is not simply removable; it is a cost worth knowing rather than a bug.

The general lesson, and the reason this is written up rather than quietly fixed: **a number that
disagrees with itself across three measurements is not a number to attribute.** The failure was
not the measurement, it was reporting a cause before the measurements agreed.

### Correction: the collatz cost is ONE variadic call, not the `math/*` wrappers

The entry above says the `math/*` wrappers cost ~40% and calls it a floor under 15 of the 31 rows.
The measurement was right; the attribution was too broad, and the isolation that settles it is one
line of `sed`:

| variant | time |
|---|---|
| `math/rem`, `math/quot`, `math/max` all qualified | 223 ms |
| the same, but `%max` called directly | **142 ms** |
| all three primitives directly | **142 ms** |

The middle row **equals** the bottom row. `math/rem` and `math/quot` are free; the whole delta is
`math/max`.

**And I read the JIT's own diagnostics backwards on the way here.** `math/rem`/`math/quot` appear
in neither `BROOD_JIT_DUMP_IR`'s lowered arms nor `BROOD_JIT_BAIL_TRACE`'s refusals, and I took
that to mean they run on the VM and the native loop pays a round-trip per arithmetic op. It means
the opposite: a fixed-arity body that is a single primitive call is **inlined into its caller and
ceases to exist**, so there is nothing left to lower or refuse. `steps` proves it — it lowers with
**no `Call` instruction at all**:

```
arm: 17 (steps)  insts: Prim2SlotInt JumpIfFalse Local Jump Prim2SlotInt Const Prim2
                        JumpIfFalse Prim2SlotInt Prim2SlotInt SelfCall Jump …
```

`math/max` is the one that shows up, as `GlobalIc Local Call`, because `(apply %max xs)` over
`& xs` allocates an argument list and cannot be inlined.

**Absence in a JIT trace is ambiguous** — it means "never hot", "refused", or "inlined away", and
those want opposite responses. `BROOD_JIT_BAIL_TRACE` exists precisely because absence from the IR
dump was ambiguous between the first two; this is the third case, and nothing distinguishes it
except reading the caller's instruction list or isolating the call.

So the lever is **variadic dispatch**, which is where CLAUDE.md's dogfooding section already points
(its worked example is variadic `+`/`-`/`=`, and its prescribed fix is multi-arity dispatch in the
evaluator rather than Rust builtins). The good news in the correction: the one-primitive inlining
already works, so there is no inliner to build — only the variadic shape it cannot reach.
## 2026-08-28 — the ordering hole: three modules that could not sort their own values

Picked up from "what is left on the library changes?" — the review had two datetime items
open. Answering them turned up five defects, and the first four were one defect wearing
different clothes: **a value that is obviously orderable, which the language could not
order.**

`(sort [dt1 dt2])` raised `compare-to: no method for [:datetime/datetime …]`. Ordering a
record routes through the `compare-to` multimethod (`%ord-compare`, the seam `sort`/`sort-by`
consult) and `std/datetime` registered no method — while carrying five hand-written
comparisons, `before?`/`after?`/`not-before?`/`not-after?`/`same?`, that were `<`/`>`/`>=`/
`<=`/`=` over `->epoch-ms`. Five public functions doing what the operators would have done,
none of them reachable by an operator. `std/tempo` had the same gap spelled differently: a
plain `tempo/compare-to` *function*, which multimethod dispatch never consults, in the module
whose whole subject is putting time on a line. One `defmethod` per type replaced all six and
bought `<`, `<=`, `>`, `>=`, `sort`, `compare` and `min`/`max` (ADR-286). Zero external
callers, so the five deleted cleanly.

Then the same probe asked `(compare dt1 dt2)` and got **0** for two different datetimes.
That one was not datetime's: **every map compared equal to every other map**, and every set
to every set — both fell through `value_cmp` to the cross-kind `tag_rank` arm, where they
rank identically. So `(compare {:a 1} {:a 2})` was 0 and `(sort maps)` returned its input
unsorted, silently. This is KI-75 again — a `compare` that calls unequal values equal and a
`sort` that no-ops rather than failing — five weeks later, on a different type, and the
`%ord-compare` docstring had been promising this order was "deterministic **and** total" the
whole time. Ordered by size then entries-in-key-order, with the entries sorted before
comparison, since a CHAMP iterates in hash order and the result would otherwise depend on
insertion history (ADR-285).

The fifth was the other open review item. `(parse-iso8601 "2026-01-02T03:04:05+02:00")`
returned **nil** — a valid ISO 8601 timestamp, of the shape every real API emits, reported
identically to garbage. The module is UTC-only and says so, but that is a statement about the
values it holds, not the inputs it can read: an offset names a real instant, so it is applied
and the UTC instant comes back. Worth recording that my first cut of this was itself the bug
it was fixing — it stripped the colon before validating the shape, so `+2:0` collapsed to
`20` and parsed as **+20:00**. A malformed offset silently becoming a specific wrong one is
worse than the nil I was removing. Caught by the test asserting it stayed nil, which I had
written before the code.

Along the way, three documentation claims that were false rather than merely stale. The
`defrecord` docstring's only example of `:derives` was `(defrecord point (x y) :derives
[Ord])` — and `Ord` **does not exist**: `(%ability-ops 'Ord)` is nil, so the one worked
example of that clause was an expansion error. The `defability` docstring illustrated
provided-vs-required ops with the same fictional ability, using an op name that is a live
multimethod. And the `%ord-compare` comment opened by saying the default is the structural
`compare`, three lines above the sentence saying there is no default — the code has always
been strict; only the prose disagreed. The `:derives` error message was also conflating "no
such ability" with "ability declares no recipe", which sends you looking for a recipe on
something that isn't there; it now distinguishes them.

219/219 `.blsp` test files (the 9 conformance/JIT-depth files are environment-gated, matched
against the pre-change baseline), 628 Rust lib tests, the zero-warning checker gate, `nest
format --check`, `cargo fmt --all --check`, and clippy `--all-targets --all-features` all
green.


### `math/max`/`math/min` get a two-argument arm — `collatz` −38.2%, and nothing in the kernel changed

The lever the isolation pointed at, taken. `max` and `min` were single-clause `(& xs)` over
`(apply %max xs)`; they now carry a two-argument arm as well:

```lisp
(defn max "…"
  ((a b) (%max a b))
  ((& xs) (apply %max xs)))
```

**The capability was already there, which is the pleasant part.** `docs/language.md` guarantees an
arity arm "binds its params *directly* (no rest-list), so it's as cheap as a single-clause fn —
this is how the prelude's variadic `+`/`-`/`<`/`=` stay fast and stay Brood", and `<=`'s own
comment records the mechanism: its 2-arg body is spelled `(%le a b)` rather than
`(not (%lt b a))` "so the ADR-069 thin-wrapper elision reaches it". `math/max`/`math/min` had
simply never been given the treatment their prelude neighbours got. So no multi-arity dispatch had
to be built — CLAUDE.md's prescribed lever turned out to be *already implemented and unused here*.

`make ab` against the parent, best-of-11, `--floor`:

| row | base | new | delta | floor | verdict |
|---|---|---|---|---|---|
| `collatz` | 228 ms | 141 ms | **−38.2%** | 0.4% | improved |
| `latency` | 4743 ms | 4589 ms | −3.2% | 2.4% | noise, same direction (the other `math/min` row) |
| `loop`, `fib`, `primes`, `sort`, `json` | — | — | within ±1.4% | — | flat |

The qualified call is now indistinguishable from the primitive — 139 ms against a `%max`-direct
control of 141 and an all-primitives bound of 140, from 223 — and `math/max` no longer appears as a
lowered arm at all, because it is elided into its caller exactly as `math/rem` is.

**Guarded and sabotage-verified.** `tests/math_test.blsp` pins the 2-arg arm separately from the
variadic path: equal args, negatives, mixed int/float, and explicit 2-arg-vs-variadic agreement.
Breaking one assertion reddens the suite and names it (5151 in-language tests, 1 failed). Pinning
the 2-arg case on its own matters because a wrong arm leaves every pre-existing many-arg test green
while changing the answer at the arity everyone actually calls.

**Left alone on purpose:** `bytes/concat` and `hash-map` are the only other single-clause
`(& rest)` wrappers over an `apply` in `std/`. Neither is shown to be hot, and adding arms on
speculation is how a tree fills up with changes nobody can attach a measurement to.

### Re-profiled the call path before starting the call-convention work — one claim confirmed, one withdrawn

`perf` is unusable on this box (`perf_event_paranoid: 4`), so this used brood's own counters. The
method that made them trustworthy is worth stating, because the first pass was wrong: **scope with
`perf/measure`, then size-sweep and keep only the counters whose ratio tracks the work.**

At whole-process scope, `pipeline` reported `alloc` and both call-IC counters *identical* between a
1× and a 10× run while `env-get` scaled 8× — every interesting counter was boot's, not the row's.
And `hof-decline-queued` at process scope reads like "the HOF fast path is declined on 96% of
activations", which is a striking and completely wrong conclusion: scoped and swept, it is a fixed
~33k warm-up cost that does not grow with the work at all. I nearly wrote it up as the finding.

**`pipeline` — the "allocation churn dominates" claim is withdrawn.** `alloc` is **15 and flat**
across a 10× sweep. `alloc_slot!` is the single macro behind `alloc_pair`/`alloc_vector`/
`alloc_map`/`alloc_closure` and the rest, so that is all LOCAL heap allocation: the lazy
`lfilter`/`lmap` form really does stream without allocating per element, which is what it exists
for. What scales is **1.48 `jit-link-done` and 1.40 `env-get` per element** — the call path. The
entry's other half ("~50% call plumbing") stands.

**`bintree` — confirmed, not corrected.** 4,095 allocations per iteration is exactly 2¹²−1, one per
node; 15,772 links per iteration is **3.85 per node**. That is an independent confirmation of the
entry's "~77 ns per node over four non-tail calls" from a counter rather than a stopwatch, so the
call-convention work is aimed at the right row. Note `vm-apply`/`env-get`/`prim2-inline` read
*fixed* here — the arm is native, so its iterations stop being counted while its allocations do
not, which is the documented `:alloc-bound` caveat and the reason not to read those columns.

Full tables in [compute-frontier.md](compute-frontier.md) §2c. Also: `make perf-brood` overwrites
the same binary path as `make release-brood`, so it was rebuilt afterwards and the "compiled out"
hint checked — timing anything against the counter build charges the change for atomics it never
introduced.

## 2026-08-28 — the checker had no opinion about `(/ x 2)`

Picked up the **merely-wider** roadmap item, and the first finding was that the entry
described its own example wrongly. It called `(/ x 2)` "a body typed exactly `number`
(int ∪ float)". It is neither: Brood's division is **exact**, so integer division yields an
`int` when it divides evenly and a **ratio** when it does not — never a float, at any arity
(`(/ 2)` → 1/2, `(/ 12 5 3)` → 4/5) or width (a bigint numerator gives a bigint-backed ratio).

And the checker was not typing it as `number` either. `/` sits in `is_contagious` but not in
`is_int_closed`, so for all-int operands the arithmetic rule fell straight through to `None`:
**no claim at all** about the most ordinary arithmetic expression in the language.

So the sound half was sitting there unclaimed. `(/ int int)` now types as `int | ratio`, which
catches a declared `float` return (`declared return type float but the body yields int |
ratio`) and a result fed somewhere non-numeric — at **no cost in false positives**, because
the case that got the item deferred is a *different* one: `int | ratio` DECLARED `int`, which
is right whenever the numerator is even and needs range analysis to prove. That stays
deferred, and it is now a narrower gap than the entry claimed — the residual ambiguity is
int-vs-ratio, not int-vs-float.

Worth recording as method, since the same shape recurs: the entry had been carrying a wrong
premise long enough to look settled, and one `type-of` probe at the REPL overturned it. The
deferral was still right; the reason given for it was not, and the useful 80% was on the other
side of that reason.

Rust tests in `check::tests` (positive, merely-wider negative, and contagion-still-wins), plus
in-language coverage in `tests/sig_adoption_test.blsp` that asserts the runtime agrees with the
claim on both arms. Sabotage-verified. Zero-warning gate over `std/` + `tests/` still clean,
207/207 `.blsp` files, 630 Rust lib tests, clippy `--all-features` clean.

## 2026-08-28 — doctests were mostly already written, and merely-wider got measured

Two of the four open items; the other two were "prove green end-to-end" (1227/1227 via
`make test-light`, the first full run over today's ~15.6k insertions plus the checker work)
and pushing.

**Doctests shipped, and were much cheaper than recorded.** The ROADMAP called this "bigger
(docstring example parser + a discovery pass)". Both already existed — as six *private*
functions inside `tests/doc_examples_test.blsp`, hard-wired to `builtin-modules` so they could
gate `std/` and nothing else. The work was extraction, not construction: lift to
`std/tool/doctest.blsp`, make the parts public, and take a prefix so a run can be scoped to
one package's surface. `nest test` now runs a project's own examples after the suite passes
and fails the run on a broken one, naming it:

    1 documented example(s) do not hold:
      proj/demo/triple: (demo/triple 4)  → got 12, documented 99

Scoped to `*project-name*` so it gates what the project promises, not its dependencies or the
stdlib; a **nameless** project is skipped rather than widened to the whole image (its modules
are unrooted, so no prefix means "mine"). `tests/doc_examples_test.blsp` now *uses* the module
instead of duplicating it — which is also the proof the extraction is faithful: sabotaging a
`std/` docstring still fails it with the identical message.

**The merely-wider residue: measured rather than argued a third time.** The deferral turns out
to be *architectural*, not an omission. The body grades as **dynamic**, so `consistent_with`
takes the `∩ ≠ ⊥` arm and `int|ratio ∩ int ≠ ⊥` passes; flagging it means switching that arm
to `⊆`. I did exactly that, temporarily, and counted:

- **zero** new warnings across all of `std/` + `tests/`;
- **4 of 5** on a probe of ordinary *correct* code — `(/ x 1)`, `(/ 6 3)`, `(/ x x)` and
  `(/ (* 2 x) 2)` are each provably int and each get flagged. Only `(/ x 2)` for an unknown
  `x` is genuinely undecidable.

So the in-repo cost is zero *only because nobody has written the code that breaks yet*, which
is the trap rather than the reassurance: the gate looks free until someone divides by 1. Not
shipped. What the measurement does give is the **order for a future attempt** — narrow first,
flag second: make the decidable cases type as exactly `int` (literal folding, a literal ±1
divisor, then parity), and only once the residual really is undecidable does flagging it
become a strictness judgement instead of a false positive. Recorded on the ROADMAP entry so
it is not re-litigated from scratch.

Worth keeping as method: "would this false-positive?" is an empirical question, and a
throwaway 10-line experiment answers it better than a third round of reasoning.

### `perf` works now, and the call protocol is confirmed — after boot faked two findings

`scripts/enable-perf.sh` + `make perf-symbols` (both added today) made per-symbol profiling
possible for the first time. `--call-graph dwarf` produced no usable chains; frame pointers
(`-C force-frame-pointers=yes`) do, and that is what resolved the attribution.

**Boot is ~47 ms, and it produced two confident wrong answers before I sized the runs properly.**
`bintree` at its benchmark size is 160 ms, so 29% of that profile is boot — appearing as
`Heap::env_get` 14.4% and `eval_tail_loop` 4.25%, which reads exactly like "this JIT'd row is 18%
interpreted". I reported that. At n=2000 both symbols vanish, and the tree-walked form count is
*identical* at n=3 and n=6 (13,253 either way): fixed startup work, zero per-node.

The real profiles:

- **`bintree` (n=2000):** `jit_run_fast_link` **24.0%**, native arms 25.1%, `__memmove` 11.4%,
  `brood_rt_fast_frame` **10.7%**, `make_vector2` 5.7%, `push_n`/`fastlink_base`/`roots_base` 8.9%.
  **Call protocol ≈ 44%, real work 25%, allocation ≈ 21%.**
- **`pipeline` (N=10M):** `dispatch` 13.5%, `jit_dispatch_call` 7.9%, `vm_cache_arm_handle` 5.7%,
  `passthrough_arm` 5.4%, **SmallVec staging 9.9%**, `Heap::closure` 4.9%, `push_frame` 4.3%.
  **~50% call plumbing.**

So FRONTIER's call-protocol framing is vindicated on both rows, and `jit_run_fast_link` alone costs
as much as all of `bintree`'s native compute. Starting there rather than at argument staging: 24% +
10.7% on one row beats 9.9% on another.

**Two of my own answers withdrawn, both from reasoning rather than measuring.** That argument
staging must be cheap because `SmallVec<[Value; 4]>` keeps ≤4 args inline — true, and irrelevant,
because the cost is the copy (9.9%, and FRONTIER's old ~8% was right). And that `env_get` was the
top cost — a boot artifact. Five contaminations in one day across three different tools, all one
cause, so the rule is now written at the top of [benchmarking.md](benchmarking.md): **measure at
two sizes and keep only what scales.** Two runs instead of one, and it is the whole difference.
