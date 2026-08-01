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

## 2026-07-24 — WASM interop slice 2: bytes marshalling (`list<u8>` ↔ `bytes`)

The next WASM slice after the ADR-145 host — a capability, not the delivery
vehicle. Before this, `list<u8>` crossed the boundary only as a vector of ints:
a `bytes` value couldn't be passed to a `list<u8>` parameter, and a byte-returning
export came back as an int vector. That blocks the canonical wasm extension shapes
(hash, compress, codec, binary parse), which are all byte-oriented.

`crates/lisp/src/wasm.rs`: `lower` grows a fast path — a `Type::List` whose element
is `u8` accepts a `Value::Bytes` and lowers each octet in one pass (a vector/list of
ints still lowers via the generic path). `lift` splits the merged `List|Tuple` arm:
a **non-empty** `Val::List` whose elements are all `Val::U8` lifts to a `Value::Bytes`
(via the blob heap) — detected from the self-describing `Val`s, so no result-type
threading. The one edge: an **empty** `list<u8>` result is indistinguishable from an
empty `list<s32>` (both `Val::List([])`), so it stays an empty vector — documented,
and a caller needing empty bytes builds one explicitly.

Copy-based (the deferred slice is zero-copy read-mapping into linear memory).
Testable toolchain-free: `tests/wasm_test.blsp`'s `*memory-wat*` gained a `byte-sum`
core func and two component exports — `blob-echo (list<u8>) -> (list<u8>)` (reuses
the string `echo` core, since a string and a `list<u8>` share the `(ptr,len)`
canonical layout) and `byte-sum (list<u8>) -> u32`. Three new tests: a byte-faithful
round-trip through high bytes (`\x00\xff\x80` — proves it isn't UTF-8-routed), the
bytes-or-int-vector lower, and the empty-list edge. 15/15 wasm tests green, GC-stress
+ verify clean. Recommended next WASM slice: the package-manager `:native`
manifest/lock/fetch integration (the delivery vehicle) — see docs/interop.md.

## 2026-07-24 — LSP tier-3: incremental document sync

The server was full-document sync — every keystroke re-sent the whole buffer. Now
it advertises `TextDocumentSyncKind::INCREMENTAL` (via `TextDocumentSyncOptions`,
`crates/lsp/src/main.rs`) and applies each `didChange` range in place: a new
`apply_content_change` splices `change.text` over the range's byte span, resolved
through the already-cached UTF-16 `LineIndex::offset` (the shared prerequisite that
already existed and was round-trip-tested). Edits within one batch compound, so the
`LineIndex` is rebuilt per edit (a single byte scan); a change with no range is a
whole-document replace. The **parse stays whole-document** — incremental *sync* only
spares the transport re-sending a large file on every edit; incremental *parse* is
still premature (the reader is cheap), so there's no new cache-invalidation logic.

No new per-document state. 2 tests added to `mod server_tests` (over the
`Connection::memory()` harness): a single ranged splice lands on the exact line
(break the middle of three clean lines, then fix it — a wrong offset would corrupt
a different line), and a two-edit batch compounds (`[` then ` nil]` → the clean
`[nil nil]`, which only results if the second edit's offset resolves against the
first's output). 116 LSP tests green.

Deferred (noted): range / delta semantic-token requests. Delta needs new stateful
machinery (resultId issuance + a previous-`data` cache on `Document` + a diff), and
the token walk already runs off a cached CST — so the payoff is marginal until
profiling shows token recompute/bandwidth actually hurts (ADR-011).

## 2026-07-24 — Telemetry metric aggregators + sampling (Elixir Telemetry.Metrics, in Brood)

The next telemetry sub-item after the ADR-137 kernel event sources — and the one
ADR-106 explicitly anticipated ("a handler that folds events into running stats").
All in `std/telemetry.blsp`, **zero new kernel surface**: `counter`, `sum`,
`last-value` (gauge), `summary` (running count/sum/sum-of-squares/min/max), and
`sample-every` (deterministic 1-in-N), plus `metric`/`metrics-snapshot`/`reset-metrics`
readers. Maps 1:1 to Elixir's `Telemetry.Metrics`.

The design leans on two existing pieces: metric state is a shared `table` (ADR-107),
and — the load-bearing observation — every telemetry handler runs SERIALLY in the one
listener process (ADR-106), so a plain `table-get`+`table-put` read-modify-write is
race-free inside an aggregator. That lets `summary` keep float-safe RUNNING aggregates
(count/sum/sumsq/min/max, mean+stddev derived on read) instead of retaining samples —
so a metric is bounded no matter how many events fire. Readers run in another process
but read the table atomically (`table-get`/`table-snapshot`). State survives a listener
restart (it lives in the table + the `def`-global handle, not listener memory) — matching
ADR-106's stateless-restartable-listener contract.

`sample-every` counts with an atomic `table-incr` (no PRNG, never loses count) and fires
the wrapped handler on every Nth event — composes with any aggregator or attach handler.

`tests/telemetry_metrics_test.blsp` (9, `:isolated`): counter/sum/gauge, summary stats
(stddev checked against 8.165 for 10/20/30), snapshot, sampling (10 emits → 3 fires), and
a concurrent-emitter fan-in (4 workers × 25 ticks → one counter reads 100, proving the
serial-listener aggregation). 9/9 + telemetry 19/19 + sysmon 8/8 green; `nest check` zero
warnings. Deferred: a distribution/histogram aggregator (percentiles need bucketing or
sample retention — a follow-up over the bounded summary).

## 2026-07-24 — LLM-native / MCP polish: watch-runtime trace tool + cookbook entries

Two ROADMAP sub-items, both pure Brood.

**MCP `watch-runtime` tool** (closes item B, "expose GC/process *traces*, not just
snapshots"). `std/tool/mcp.blsp` gains `mcp-watch-runtime-tool`: it arms the kernel
`system-monitor` on the handler process for a bounded window (`:ms`, capped 5 s,
optional `:filter` kind selector), sleeps, disarms, and drains the collected
`[:system kind pid detail]` events into `{:events [{:kind :pid :detail}] :count :ms}`.
That's the runtime-event STREAM — GC pauses, spawn/exit churn, JIT deopts — that a
point-in-time `processes`/`node` snapshot can't show. Self-contained: the global
single-subscriber monitor is armed-and-disarmed within the one call, and self-events
are never emitted so the watcher only sees other processes' activity. 21 tools now.
`tests/mcp_test.blsp` +3 (`:isolated`, since it arms the global monitor): spawn/exit
collection over a window, the `:filter` selecting only `:exit`, and the catalogue
entry.

**Cookbook entries** (item A — the intent→idiom `find-pattern` catalogue in
`std/tool/explain.blsp`). The E-code table has zero gaps (all 16 codes covered), so
the open surface was cookbook entries: added five confirmed Clojure/Scheme reflexes —
keyword-as-fn `(:k m)` → `(get m :k)`, char literal `\c` → 1-char string / `int->char`,
discard `#_` → `;`, regex `#"…"` → `(require 'regex)` + `regex/match?` — and updated
the `#{…}` set entry (was "no set literal") to the now-first-class kernel set. Reader
hints for the three that still silently mis-parse (`#_`, `#"…"`, `\char`) are a Rust
`read_hash` change, deferred.

explain 9/9, mcp 13/13, `nest check` zero warnings.

## 2026-07-24 — Finish-the-partials: reader hints (`#_`/`#"…"`/`\c`) + telemetry histogram + node-liveness stream

Closing the completable polish on three 🟡 threads (the deferred sub-items behind
each — mailbox bounds, the WASM `:native` pkg slice, dist `terminate`/FQDN/Windows —
stay ADR-011 consumer-gated, not built).

**Reader hints — the last three silently-mis-parsed Clojure/Scheme forms** (closes the
LLM-native reader-hints thread; the cookbook already named these idioms 2026-07-24).
`read_hash` (`crates/lisp/src/syntax/reader.rs`) gains `#_` → hint the `;` comment
idiom (no form-level discard) and `#"…"` → hint `(require 'regex)` + `regex/match?`
(the `#b"…"` bytes literal is matched *before* the dispatch, so a `#"` is unambiguously
the regex form). And a new `'\\'` arm in `read_form` catches a leading `\c`/`\newline`
character literal — which previously read as a stray symbol `\c` → "unbound symbol: \c"
— with a hint naming the 1-char string / `int->char` idiom (Brood has no char type; a
leading backslash is never a valid form start, and a repo-wide grep confirmed no
existing source relies on one). `tests/reader_hints_test.blsp` +3 (14 total, all green),
`docs/language.md` "Coming from Clojure" table +3 rows.

**Telemetry distribution/histogram aggregator** (the one named follow-up over `summary`;
`std/telemetry.blsp`, zero new kernel surface). `distribution` buckets a measurement
into explicit ascending upper bounds (Prometheus / Elixir-`Telemetry.Metrics`-
`distribution` shape) — per-bucket counts + count/sum/min/max, **bounded** (no samples
retained), matching `summary`'s running-aggregate philosophy. `(metric id)` presents
`:buckets [{:le b :count c}… {:le :inf :count c}]` + `:mean`; `metric-percentile` estimates
a quantile by linear interpolation within the containing bucket (`histogram_quantile` —
bounded memory for approximate quantiles). Unsorted bounds are normalized ascending.

**Node up/down through the telemetry stream** (`watch-nodes`, `std/telemetry.blsp`).
The kernel has no `[:nodeup]` event — `monitor-node` fires only `[:nodedown]` — so a
general watcher can't be purely event-driven; `watch-nodes` polls `(nodes)` and diffs
consecutive peer sets, re-emitting each change through the SAME `[:runtime kind]` seam
as `watch-runtime`: `[:runtime :nodeup]`/`[:runtime :nodedown]` with `{:node name}`.
Polling (second-scale, `:interval-ms`) suits node liveness — rare operational events —
and catches BOTH inbound peers and outbound `connect`s, which a per-spec `monitor-node`
alone would miss.

`tests/telemetry_metrics_test.blsp` +2 describes (15 tests, 6 `:isolated`, all green):
distribution bucketing/overflow/normalization + percentile interpolation (p50→10, p90→18
on a two-bucket fold), and node liveness — the peer-set diff (via `(:use-internals
telemetry)`, the ADR-146 @testable seam), the up/down emit seam, and `watch-nodes`
lifecycle with no spurious events on a single node. A live two-node cluster rides the
dist suite. `nest check` zero warnings.

## 2026-07-24 — Package manager v2: tarball deps + a git-backed registry (ADR-147)

Finished the two ADR-037-deferred package-manager pieces (the concrete pull: a request
to complete both). One new Rust primitive; everything else Brood policy.

**`:tarball` deps** (`[name :tarball URL :sha256 HEX]`). Download via `std/net`'s
byte-faithful `http-get` (no new Rust HTTP client — `std/net` post-dates ADR-037's
planned `%http-get`), or read a `file://` path directly (offline artifacts + the
offline test path); http(s) follows bounded redirects. `:sha256` is **mandatory** and
verified before extraction (ADR-037's supply-chain property kept) — a mismatch is a
loud error. The one new primitive is **`%untar-gz`** (`crates/lisp/src/builtins/io.rs`),
a thin shell to `tar` like `%git-clone` shells `git`, on the ADR-144 offload allow-list,
stripping the single wrapper dir. Wired through resolve/lock/conflict/tree/CST/`add`.

**Git-backed registry.** The index is just a git repo of `packages/<name>.blsp`
metadata — no hosted server (ADR-037's "no central infrastructure" kept). `nest publish`
appends the project's `{:version :git :ref :description}` entry to a local index
checkout and stops (no auto-commit — the user owns the index repo: review, commit,
push). `nest search` greps it. A `[name :version "X.Y.Z"]` dep resolves the **exact**
version to its git source and pins it, reusing the whole `:git` cache/lock path. Two
new optional manifest fields feed publish: `:description`, `:repository`. A `:registry`
config key (default `github.com/broodlang/registry`) sets the index. No semver solver
(ADR-037 invariant). New `nest publish`/`nest search` subcommands (`crates/nest/src/main.rs`).

`tests/package_test.blsp` +9 (4 tarball: strip-extract, sha-mismatch guard, cache hit,
parse; 5 registry: append/duplicate, required fields, search/find, `:version` resolve +
lock-vec shape, parse) — all offline via `file://` + local git repos; 39/39 green. The
cwd-driven verbs (`publish`/`search`) are hand-verified via the binary (per the repo
convention that cwd verbs aren't Rust-E2E'd; their arg-driven cores ARE unit-tested).
`nest check` zero warnings.

## 2026-07-24 — File-organization pass: split the giants, no behavior change

A pure code-organization sweep (no logic changes) breaking up the largest,
multi-concern files and fixing two placement/naming issues. Every step verified
green (build + suite; the two hot-path splits additionally via `make test-both`).

**Test extractions** (inline `#[cfg(test)] mod tests` → sibling files; the dominant
line reductions): `types/check.rs` 4539→830 (`check/tests.rs`+`soundness_oracle.rs`),
`types/mod.rs` 2768→1435 (`tests.rs`; also peeled `sig.rs`/`display.rs`),
`dist/wire.rs` 1245→902 (`wire_tests.rs`), `nest/mcp.rs` 2386→1058 (`mcp_tests.rs`),
`lsp/main.rs` 1587→970 (test files + a new `uri.rs`).

**Hot-file splits** (child-module layout — `use super::*` + `pub(crate)` sweep +
`pub(crate) use child::*` re-exports; behavior-identical relocations):
- `core/heap.rs` 10760→3912, adding `heap/{gc,map_ops,equality,vm_cache}.rs`. Children
  of `heap` (not siblings) so they reach `Heap`'s private fields/methods/structs with no
  visibility churn; a few private methods called cross-sibling widened to `pub(crate)`;
  `stall_threshold_ms`/`stall_guard_pid` re-exported. GC-suite verified.
- `eval/compile/mod.rs` 9569→2157, adding `emit/exec_value/dispatch/exec_chunk/vm_run_bc/
  inline/jit_runtime/tests.rs`. `jit_runtime.rs` is `#![cfg(feature="jit")]`; jit +
  non-jit builds both clean; differential (both-engine) gate green.

**Builtins coarse splits** (glob-import architecture keeps `register()` untouched):
`system.rs` 2909→2411 (`selfhost_macros.rs`/`tooling.rs`/`errors.rs`), `io.rs` 2093→1929
(`os.rs`). The remaining internal giants (system's self-hosting + processes blocks; io's
git/crypto/fs braid) left as follow-ups — unmarked internal boundaries / heavy interleave.

**Brood std:** `std/tool/project.blsp` 2436→1358, scaffolding (templates + `new-project`)
split to `std/tool/scaffold.blsp` — a new bundled module `(:use project)` for
`*config-git-init*`; `nest new` re-pointed to `scaffold/new-project`; smoke-tested.

**Placement/naming:** `proc.rs`→`subprocess.rs` (naming hazard vs `process.rs` — the two
were unrelated subsystems); `table.rs`→`core/table.rs` (peer of `core/blob.rs`/`map_champ.rs`).

Skipped/deferred with rationale: externalizing `random` from the prelude (user-facing
break for ~64 lines); `net.rs`/`gui.rs`/`jit_lower.rs` deep splits (tightly-coupled
delicate code, marginal-vs-risk); std crypto/data grouping (bundled names are
path-independent, so purely cosmetic — ADR-011 flat-is-fine).

## 2026-07-24 — Structural cleanup Tier 3 (quick wins) + a broken-build fix

The safe/mechanical items from the structural review (ROADMAP "Structural /
code-organization cleanup"), plus a build breakage found on the way.

**Build fix:** `gui.rs` referenced `crate::core::heap::stall_guard`, but after the
`heap/gc.rs` split only `stall_guard_pid`/`stall_threshold_ms` were re-exported
from `heap` — added `stall_guard` to `pub(crate) use self::gc::{…}` (a
gui-feature-only build failed without it).

**Dead code:** deleted `cli_support::parse_jobs_args` (clap replaced it) and
`Scanner::set_pos` — both zero callers.

**Doc-comment misattachments:** the crash-dump doc now sits on
`install_crash_dump` (was stranded above `fmt_utc_ms`); the `syntax/cst.rs`
string paragraph moved onto `fn string` (was above `fn hash`); removed the
orphaned `mailbox-size` doc left in `terminal.rs` (the primitive is documented in
`PRIMITIVE_DOCS` + `process/mailbox.rs`).

**Stale headers:** `builtins/io.rs` "terminal frontend (ADR-046)" banner (over
`mailbox-size`/`process-info`) → "process introspection (ADR-051)"; fixed the
first-line path comments in `std/editor/ansi.blsp` and `std/net/http.blsp`.

**std/ consistency:** added `scaffold`'s missing `defmodule` docstring; renamed
the `treesit` module → `editor/treesit` to match its file/registration/require
path (updated the `treesit/`→`editor/treesit/` call sites in
`tests/treesit_module_test.blsp`); moved `std/agent.blsp` → `std/proc/agent.blsp`
beside `gen`/`supervisor`, renamed the module `agent` → `proc/agent`, and updated
the registration in `system.rs`, the benches, and `tests/agent_test.blsp`.

Verified: `cargo build -p cli` with gui+treesit-grammars+jit clean; full Brood
suite 2979/2979 green.

## 2026-07-24 — `:format-plugins` now resolves any dep kind, not just `:path`

**Bug:** `:format-plugins [dep]` was a silent no-op for every dep kind except
`:path`. `project--plugin-format-headers` went through `project--path-dep-dir`,
which matched `(= (get d :kind) :path)` and returned `nil` for anything else — so a
project that consumed a framework as a `:git` dep lost the framework's declared
`:format-headers`, and `nest format` reflowed the framework's macros against its
declared shape (a `hatch` consumer's `(on "increment" (params model) …)` clause
signature dropped onto its own line). Nothing warned; formatting just regressed the
moment a dep was switched from `:path` to `:git`.

**Fix:** replaced `project--path-dep-dir` with `project--dep-manifest-path
(dep-name root)`, which resolves the dep's own `project.blsp` for *every* kind — a
`:path` dep in place at its `:path`, a fetched dep (`:git`/`:tarball`/`:registry`)
from its `_deps/<name>/` checkout (`package--git-target`'s layout; a `:registry`
dep resolves through the same git path). Ordering already works out:
`project-setup` runs `project--ensure-deps-on-path` (which fetches into `_deps/`)
before it computes `*format-headers-extra*`, and
`project--manifest-format-headers` returns `{}` for a missing file, so an
unfetched/offline dep degrades to no extra rules instead of erroring.

Tests in `tests/project_test.blsp` ("project: :format-plugins manifest
resolution"): path/git/tarball/absent resolution, a `:git` plugin's headers
reaching `project--effective-format-headers` with the project's own
`:format-headers` winning a clash, and the unfetched-plugin degradation.

**Formatter-canonical tree:** this surfaced that the tree had drifted out of the
current formatter's canonical form — a whole-tree `nest format` rewrote ~202 of 262
files (trailing comments migrating off their form, continuation lines re-indenting).
Closed in `96f9bfd` as a deliberate one-shot reflow (`nest format` + `cargo fmt`),
so `nest format` is a no-op again and the checklist's whole-tree run is safe.

**Follow-up in the same area:** `:format-plugins` naming a dep that isn't in
`:dependencies` now **warns** to stderr instead of silently contributing `{}` — the
same silent-failure class as the bug above (a typo'd or dropped plugin name would
otherwise reformat the project against rules it only thinks it pulled in). A
declared-but-*unfetched* dep stays quiet: that's the ordinary offline case, and
warning on it would be noise. The warning can't be asserted in-language
(`with-out-str` captures stdout, `eprintln` writes stderr), so the tests pin the
behaviour instead: a bogus entry contributes nothing *and* doesn't cost a
correctly-declared sibling its rules.

## 2026-07-24 — Structural cleanup Tier 2 (dedup)

The dedup items from the structural review (ROADMAP "Structural /
code-organization cleanup", Tier 2). 5/6/9 + the parse-url half of 7 landed; 8
and the path half of 7 deferred with rationale.

**Item 9 — `eval/compile/inline.rs`.** Collapsed the near-identical
`node_has_selfcall` (non-gated) / `node_has_self_call` (jit) /
`node_has_make_closure` (jit) into one generic `node_any(node, &pred)`
"does the tree contain a node matching pred?" combinator.

**Item 6 — `lib.rs` `eval_str`/`eval_source`.** Factored the shared top-level
driver into a private `eval_forms(Vec<(Value, Option<Pos>)>)` that carries the
delicate GC-rooting (root the unevaluated forms, re-fetch via `root_at` across a
collection), namespace pre-scan, and per-form reset logic exactly once; the two
public fns are 3-line adapters (no positions → `None`; positioned → `Some`, which
gates `note_definition` + `or_pos`). Restore now runs once on all paths.

**Item 5 — `types/mod.rs` literal refinement.** Four generic helpers
(`merge_union_lit_set`/`intersect_lit_set`/`lit_is_subtype`/`lit_disjoint`, over
`T: Ord + Clone`) replace the per-kind (`Symbol`/`i64`/`bool`/`String`) blocks in
`union`/`intersect`/`is_subtype`/`is_disjoint`/`negate`; all ten `Ty`
constructors now use struct-update over `Ty::flat(tags)`. ~250 lines out,
behaviour identical (238 lattice/checker Rust tests + `nest check` green).

**Item 7 (url half) — `std/net/http.blsp`.** `parse-url` was a lossy reimpl of
`url/parse-url`; it now `(require 'url)`s and wraps the one RFC-3986 parser,
applying HTTP defaults (scheme http, port 80/443, path "/") and prepending
`http://` to a scheme-less input so a bare `host[:port]/path` still parses as an
authority. `:use url` would clash on the `parse-url` name, so it's a qualified
call.

**Item 7 (path) — resolved as keep-both.** `path.blsp` (the full path API) and the
prelude `path-*` bootstrap subset aren't true duplication: the subset must exist
before any module loads, and the two carry different contracts (`path/basename`
strips a trailing slash, `path-basename` doesn't; `path/join` is variadic +
absolute-reset, `path-join` is 2-arg). Documented the layering in both files so it
isn't re-flagged; no code churn.

**Deferred:** item 8 (`gui_gpu.rs` is a prototype — the missing ops are
unimplemented GPU features, not diverged geometry; needs a live display to
verify).

En route: fixed the `heap::stall_guard` re-export I added on 2026-07-24 — the
*function* is used by the GC (always compiled), only the `heap::` re-export is
gui-only, so the re-export (not the fn) is now `#[cfg(feature = "gui")]`.

Verified: suite 2985/2985; `nest check` zero warnings; types/compile/interp Rust
tests green; jit + non-jit + gui feature configs all compile.

**make install warning-free (same session).** The `make install` build
(`release-fast`, gui+treesit-grammars+jit) emitted 5 `private_interfaces`
warnings: `pub(crate)` VM functions (`exec_call`/`dispatch`/`exec_chunk`/
`attach_vm_trace`/`jit_dispatch_tail`) exposed `pub(super)`/private types
(`Step`, `ChunkExit`, `BcFrame`) in their signatures. Since `eval::compile`
re-exports its children crate-wide (`pub(crate) use child::*`), the consistent fix
is to widen those three types to `pub(crate)`. `make release` (cli + nest +
brood-lsp) now builds with zero warnings.

## 2026-07-24 — Structural cleanup Tier 1 item 4: PRIMITIVE_DOCS drift guard

`register()` and `PRIMITIVE_DOCS` sit ~2000 lines apart in `builtins/mod.rs` and
agree only by string key, with no test — a new primitive (or a rename) could
silently lose its doc. Added a unit test (`builtins::primitive_docs_tests`) that
registers every primitive into a fresh LOCAL env, enumerates the natives, and
asserts: (1) every user-facing (non-`%`) primitive has a `PRIMITIVE_DOCS` entry;
(2) no doc entry is an orphan (a name nothing registers). `%`-prefixed ops are
internal (wrapped by a prelude fn/macro) and exempt.

The guard immediately caught 12 undocumented user-facing primitives —
`bytes`/`byte-at`/`byte-length`/`bytes->list`/`bytes-concat`/`bytes-index-of`/
`subbytes`, `max`/`min`, `current-ns`/`seqview?`/`demonitor-node` — docs added
(verified they now surface via `(doc …)`). 0 orphan docs.

## 2026-07-24 — `nest test` selection: `mix test` parity

**The gap:** with 2965 tests the only way to narrow a run was a file path — no
name filter existed anywhere, not on the CLI and not in `std/tool/test.blsp`
(`run-project-tests` forwarded only `:slow`). Iterating on one `describe` meant
running the whole suite or remembering its file.

**Shipped**, as `mix test`'s surface mapped onto the runner:

| flag | meaning |
| --- | --- |
| `--only` / `--exclude` / `--include SELECTOR` | repeatable; a tag, `test:substr`, or `describe:substr`. Several `--only`s union; `--include` beats `--exclude` |
| `FILE:LINE` | the test covering that line (last test declared at/before it) |
| `--failed` | last run's failures, from the project cache dir |
| `--seed N` | shuffle order, seed echoed in the summary for replay |
| `--partitions N --shard K` | stable-hash CI shards |
| `--max-failures N` | abandon the run after N failures |
| `--repeat-until-failure N` | up to N passes, stop at the first failure |
| `--timeout MS`, `--slowest N`, `--no-trace` | expose knobs the runner already had internally |

Tags are new: `:tags [kw …]` on `describe`/`test`, alongside `:serial`/`:isolated`
in any order, merged group→test. The per-test tuple grew from `(name thunk)` to
`(name thunk meta)` with `{:tags :file :line}`; the line comes from the first body
form's read position (captured at expansion time like `fail-loc`), which is what
makes `FILE:LINE` addressable.

Division of labour follows ADR-006: **all** selection logic and selector *parsing*
live in Brood (`test--make-filter`, `test--select-units`), so the grammar has one
definition and `nest` only forwards argv.

**Three things worth remembering:**

1. **`--max-failures` had to be enforced twice.** Per-file scoping means every file
   gets its own driver, so `run-driver`'s between-steps check only ever bounded one
   file — a 4-failure file blew straight past `--max-failures 1`. `drain-files-scoped`
   now carries a running failure count in its fold state and stops loading further
   files. The budget is still checked at step/file boundaries, so it bounds a run
   rather than stopping exactly.
2. **The shard hash needs a finalizer.** A plain polynomial hash (`h*31 + c`) leaves
   the low bits unmixed — its parity is just the character-sum parity — so
   `(rem h 2)` sent an entire small suite to one shard and left the other empty.
   Now FNV-1a masked to 32 bits with an xor-shift finalizer; the test asserts the
   spread over 200 labels, not just that shards partition.
3. **An unresolvable `FILE:LINE` must select nothing, not everything.** Deriving the
   "is anything whitelisted?" test from the *resolved* line list meant a typo'd path
   resolved to empty → no positive constraint → the whole suite ran. In CI that is
   indistinguishable from success. It now uses the *requested* `:lines`, runs zero
   tests, and warns. Caught by the new tests, not by hand.

`tests/test_selection_test.blsp` — 54 cases over synthetic unit lists (registering
real fixtures would pollute the suite being run), including an `:isolated`
cross-process block proving a filter spec survives a `send` deep-copy.

Also fixed stale docs: `docs/testing.md` claimed a 30 s default per-test timeout;
it has been 120 s.

**Not mapped from `mix test`** (recorded on the roadmap rather than silently
dropped): `--cover` (no coverage mechanism exists — the IR already carries
positions and `Value::Table` can aggregate across processes, but it wants a Rust
recording seam + a Brood reporting module + an ADR), `--stale` (needs a per-test
dependency graph), `--formatter`, `--breakpoints`.

## 2026-07-24 — review pass on `nest test` selection: four defects fixed

A deliberate review of the code from the previous entry, probing edge cases rather
than re-reading. Four real defects, all now covered by tests:

1. **Path matching crossed filename boundaries.** `test--same-file?` compared paths
   with a bare `ends-with?` in both directions — and `"extra_test.blsp"` genuinely
   *does* end with `"a_test.blsp"` (the last 11 characters are exactly that), so a
   `FILE:LINE` selector could pick a test out of an unrelated file. Now the suffix
   must land on a path-component boundary (`test--path-suffix?`: the character
   before the match has to be `/`). Narrow in practice — it needs both files named
   in one run with bare-filename paths — but silent and wrong when hit.
2. **`--shard K` out of range silently ran nothing.** `--partitions 2 --shard 5`
   matched no test and exited 0. A CI job reads that as green. Now exits 2 with the
   valid range.
3. **`--shard` without `--partitions` was silently ignored** — it ran the whole
   suite while looking like a shard. Also exits 2 now.
4. **`--seed 0` contradicted its own help**, which claimed 0 meant declaration
   order; the runner only skips shuffling when the seed is *absent*. Help corrected
   — any value shuffles, omitting the flag keeps declaration order.

Also documented that positive selectors **union**: `--failed --only slow` is
(failures ∪ slow), not the intersection. Narrowing wants `--failed --exclude slow`.

Two sharp edges noted and deliberately left:
- `test--resolve-lines` warns per *drain*, and the scoped runner drains per file, so
  a `:lines` spec reaching the scoped path would warn once per file. Unreachable
  from the CLI (a positional `FILE:LINE` forces the single-file path, one drain), so
  it stays a latent note rather than a fix in search of a caller.
- Group→test tag merging can duplicate a tag present at both levels. Matching is
  by presence, so it is harmless.

## 2026-07-24 — Structural cleanup Tier 1 item 2 (partial): scheduler guards split

Mapping `scheduler.rs` for the roadmap's preempt/pool/lifecycle carve showed the
**reduction budget** (`REDUCTIONS`/`reduction_budget`) is the shared heart of the
engine — touched by the preemption core, the capture-driver glue (`tick_capture`/
`yield_now`), and the worker loop (`finish_quantum`). A clean 3-way carve would
need accessor functions threaded through that shared state, in the KI-1
concurrency code. Not worth the risk for a file-organization pass.

Took the low-risk win: extracted the genuinely self-contained execution guards —
`GC_BLOCK`/`MACRO_BLOCK`/`STACK_BASE` thread-locals + `gc_block_*`/`macro_block_*`
accessors + `GcBlockGuard`/`MacroBlockGuard` + the stack-overflow byte guard
(`WORKER_STACK_BYTES`/`stack_budget`/`stack_overflow_check`) — into
`process/scheduler/guards.rs` (2080 → 1824 in the root; guards.rs 269). The
thread-locals are touched only by their own accessors, and `install_ctx` resets
them through those accessors, so the move is closed and behaviour-identical. The
public items are re-exported from `scheduler.rs`, so every `crate::process::…`
call site (eval, macros, cli_support, tests) is unchanged.

The reduction/worker/lifecycle engine stays one unit in the root; a further
pool/lifecycle carve (which needs the accessor layer) is deferred.

Verified: compiles default + no-default-features; full suite 3024/3024.

## 2026-07-24 — `nest test --cover`: function coverage with zero kernel support (ADR-148)

The last `mix test` flag worth having. Line coverage is the obvious reading, and it
needs a VM seam — so this ships the **function-level** tier instead, and it turns
out to need **no kernel support at all**.

`nest test --cover` reports which project functions the suite never entered;
`--cover-min PCT` fails the run under a floor (implies `--cover`). Coverage prints
even when the suite is red — that's when you most want it — and the floor is gated
*after* the suite result so a failing test reports itself rather than being masked.

**`std/tool/coverage.blsp` is pure Brood policy** over three things the language
already had:

- `global-names` + `source-location` → the denominator (functions defined under the
  project's `:source-paths`; macros/natives/data excluded, so std can't inflate it).
- `def` rebinding + late binding (ADR-013) → the instrumentation. Each target is
  rebound to a shim that records a hit and forwards; late binding means every
  already-loaded caller, in any process, picks it up with no reload.
- `Value::Table` (ADR-107) → hit collection. Tests run across processes with
  separate heaps; a table is shared by identity and `table-incr` is atomic, so
  parallel tests can't lose an update. The sanctioned mutable structure doing
  exactly its job.

Adding a VM coverage mode would have been building machinery to avoid using the
language — the ADR-006 principle read honestly.

**The one real design constraint: the shim must be variadic.** `arglist` reports
only ONE arm of a multi-arm function, so an arity-preserving shim built from it
would silently break the arities it never saw — `(defn f ((x) …) ((x y) …))` reports
`(x y)`, and `(f 1)` would then fail. `(fn (& args) … (apply original args))` is
correct for fixed, `&optional`, `& rest`, and multi-arm alike;
`tests/coverage_test.blsp` pins each shape. The cost is that every rebind
legitimately changes arity, which tripped the hot-reload diagnostic once per
function — hence one new off-switch, `BROOD_NO_RELOAD_DIAG=1` (default stays on, so
an *accidental* mismatch is still surfaced), which `--cover` sets for its own
process.

Two limits documented rather than papered over: a self-recursive tail call is
counted **once** (the VM's `SelfCall` bypasses global lookup, and so the shim), so
hit counts are a lower bound and not a profile; and instrumentation defeats JIT
inlining of the wrapped call, so `--cover` is never a benchmark.

Verified: 57% on a 7-function fixture with the 3 uncovered ones named in source
order (an earlier version listed them alphabetically — 7, 15, 9); exactly 50% of a
generated 400-function project where every even-indexed function is called, in
1.4 s; and a clean "no functions were instrumented" (returning 100%, so no spurious
gate failure) in this repo, which declares no `:source-paths`.

Line coverage stays deferred with its shape settled — see ADR-148 and
`docs/coverage.md`.

## 2026-07-24 — Structural cleanup Tier 1 item 1 (partial): extract jit_lower/i64.rs

The roadmap's target is decomposing `jit_lower_arm_inner` (a ~2,185-line Cranelift
lowering function) — the high-risk core, where a subtle miscompile passes tests
but is wrong on an edge case. Deferred that; took the clean, low-risk win instead.

Extracted the **unboxed i64/f64 scalar worker** — `Scalar`/`impl Scalar`,
`arm_scalar_kind`, the `i64_*` eligibility/op helpers, `I64Ctx`, `i64_guard_overflow`,
`lower_i64_arith`/`lower_i64_value`/`lower_i64_cond`, and `jit_lower_i64_arm`
(~860 lines) — into `eval/compile/jit_lower/i64.rs`. Verified the cluster is used
only by the `jit_lower_arm` dispatcher and the tiering glue (`jit_runtime`), never
by `jit_lower_arm_inner`, so it's a self-contained relocation. `jit_lower.rs`
5437 → 4576; i64.rs 869. `jit_lower_i64_arm` bumped to `pub(super)` (dispatcher
calls it); `arm_i64_eligible`/`arm_i64_too_deep`/`i64_mark_too_deep` re-exported
`pub(crate)` so `jit_lower::…` paths in `jit_runtime` are unchanged. The module +
re-exports are `#[cfg(feature = "jit")]` (the whole cluster is jit-only).

The `jit_lower_arm_inner` decomposition stays open for a dedicated session.

Verified: compiles default + no-default-features; differential 2/2 (JIT≡VM
bit-identical), jit 34/34, jit_runtime_compaction 3/3 — all with BROOD_JIT_VERIFY=1;
full suite 3057/3057.

## 2026-07-24 — hardening pass over the whole `nest` surface

Asked to get the session's `nest` work to 100%, so: probe every input rather than
re-read the code. Seven fixes, one of them a crash.

**A panic on `--partitions 0`.** `validate_shard`'s range message computed
`total - 1` on a `u64`, which underflows — so a bad *flag value* produced a Rust
panic, a backtrace, and a `.brood_crash_dump`. The shallow fix is a `Some(0)` arm;
the real fix is that CLI validation shouldn't be hand-rolled arithmetic at all.
All numeric test flags now carry a declarative
`value_parser!(u64).range(…)` — `--partitions`/`--max-failures`/
`--repeat-until-failure`/`--timeout`/`--slowest` ≥ 1, `--cover-min` **0–100**
(it previously accepted 150, which could never pass) — so an out-of-range value is
rejected by the parser with a consistent message and never reaches arithmetic.
`saturating_sub` remains as defence in depth. Negatives were already fine: clap
rejects them at parse time.

**Project-scoped commands leaked internals.** `nest test|check|format|doc|fetch|
tree|update|publish|add|remove|run|mcp` outside a project each surfaced a raw Brood
error — a bogus source position pointing into the bootstrap string (`1:58`), an
internal function name (`project/run-project-tests`), an internal line number — for
what is only a wrong-directory mistake. They now go through a `require_project`
guard at the `nest` boundary: one line naming the cwd, one naming the fix, and for
commands with a file-scoped form, a third naming that (`nest test <file>_test.blsp`).
Exit 2. Modelled on `nest repl`'s existing good message and on cargo's "could not
find `Cargo.toml`". The file-scoped forms (`nest test FILE`, `nest check FILE`,
`nest run FILE`, `nest doc --all`, `nest doc <module>`) are all still allowed
outside a project — verified, since guarding them would have been a regression.

**A filter matching nothing passed for success.** Found by using the tool: a stale
`--failed` record still named tests from a file I'd deleted, so `nest test --failed`
selected zero tests and exited 0. Same class as the `FILE:LINE` bug fixed earlier
in the day, so it got the same general treatment — `--only`/`FILE:LINE`/`--failed`
now warn when they match nothing. Sharding is deliberately excluded: an empty shard
is normal when a small suite fans across many machines.

**`--seed` only shuffled within a file.** The scoped runner drains file-by-file, so
files always ran in discovery order and a cross-file order dependency could never
surface — most of the point of a seed. `drain-files-scoped` now shuffles the loader
list too. Also corrected the summary line, which said "replay with --seed N": the
seed fixes *scheduling* order (verified stable across 5 runs per seed), but parallel
tests genuinely interleave, so promising exact replay was a lie. It now says "same
order … parallel tests still interleave".

**Three smaller ones.** `--slowest 0` printed a "Slowest 0 tests:" heading with
nothing under it. `project--coverage-finish!` took an `opts` it never used.
`coverage-begin!` leaked its hit table on a second call — a table lives in a
runtime-wide registry, so dropping the handle doesn't free it, and a long-lived
`nest mcp` image instrumenting twice would strand one store per run; it now
`table-drop`s the previous one.

14 new tests (`test_selection_test.blsp` 59 → 73). Investigated and left alone:
negative CLI values (already clean), `--include` without `--exclude` (a documented
no-op, not worth a warning), and the per-file `:lines` warning storm (still
unreachable from the CLI).

## 2026-07-24 — Tier 1 item 2 continued: extract scheduler/lifecycle.rs

Extended the scheduler split (after guards.rs) with the low-risk statics-in-root
approach: the shared scheduling state stays in the root, and only the **process
lifecycle** functions move — `spawn`/`spawn_linked`/`spawn_impl`/
`spawn_root_program`, `exit`/`exit_propagate`/`exit_with`, `deregister`,
`proc_descr` — into `process/scheduler/lifecycle.rs`, reaching the root's statics
and pool fns via `use super::*`. A pure relocation (statics don't move → behaviour
identical). Root 1835 → 1545; lifecycle.rs 299.

Two adjustments the move forced: `proc_descr`/`deregister` bumped to `pub(super)`
(the root worker loop calls them); `exit_propagate` to `pub(crate)` (it was
`pub(super)` = pub-in-`process`, which the re-export from the deeper module needed
widened for `process::links`); and `super::sysmon::…` paths rewritten to
`crate::process::sysmon::…` (in the child, `super` is now `scheduler`, not
`process`). Public surface (`spawn`/`exit`/…) re-exported from `scheduler.rs` so
every `crate::process::…` call site is unchanged.

The worker-pool machinery (queue/stealing/worker loop) stays in the root — carving
it needs the accessor layer for the shared reduction budget, deferred.

Verified: compiles default + no-default-features; full suite 3071/3071.

## 2026-07-24 — Tier 1 item 2 complete: extract scheduler/pool.rs

Finished the scheduler carve. The unlock was recognising the statics-in-root
approach needs **no accessor layer** (the earlier worry): keep every shared static
in the root and relocate only functions — children reach the state via
`use super::*`, and since statics don't move the split is behaviour-identical.

Extracted the worker pool + run-queue execution loop — `enqueue`/`wake_enqueue`,
`try_steal`/`try_steal_any`, `spawn_overflow_drainer`/`overflow_drain`,
`ensure_workers`/`worker_loop`/`run_one`/`finish_quantum`/`handle_capture_outcome`/
`park_on_receive`, `set_test_no_workers`/`test_drive_quanta` — into
`process/scheduler/pool.rs` (two cuts around the `TEST_NO_WORKERS` static, which
stays in root with its root callers). `assign_worker`/`pick_spawn_worker`/
`worker_count` + all module statics stay in the root. Root 1545 → 1088; pool.rs 479.

Visibility: `enqueue`/`wake_enqueue`/`ensure_workers`/`spawn_overflow_drainer`
bumped to `pub(crate)` (called from lifecycle, `process::mailbox`, and the root's
`DirtyBlockGuard`), re-exported so `scheduler::…`/`process::…` paths are unchanged.
No `super::`-path fixes were needed (pool used bare module aliases via `use super::*`).

Net: `scheduler.rs` 2080 → 1088, carved into `guards.rs` (269) + `lifecycle.rs`
(299) + `pool.rs` (479); the root keeps the reduction/preempt core, capture-driver
glue, `Process`/`Ctx`, and the shared statics.

Verified: compiles default + no-default-features; full suite 3071/3071.

## 2026-07-24 — Tier 1 item 1 cont.: start jit_lower_arm_inner decomposition (prepass)

Began decomposing `jit_lower_arm_inner` (the ~2,185-line Cranelift lowerer) by
extracting its pure, Cranelift-free **pre-lowering analysis** — the block-leader
detection + operand-stack-depth abstract interp — into
`eval/compile/jit_lower/prepass.rs::block_analysis(code, len) -> (is_leader,
depth)`. Data in, data out, emits no CLIF, so behaviour-identical by construction.
The function drops ~90 lines and the analysis now has a name + a home (the
roadmap's `jit_lower/prepass.rs`).

The emit-loop remainder (the ~1,730-line `for ip in 0..len` CLIF loop) is the
high-risk core and stays open. Assessed in detail: the 15 match arms (Call ~300,
Prim1/2/3 + fused prims ~700, SelfCall ~200, control ~130) all share `b` (the
`FunctionBuilder` — can't move into a struct, it borrows `ctx.func`), the
virtualized `Op` stack, ~30 `FuncRef`s, and the hoist maps. Extracting any family
needs a pervasive `LowerCtx { stack, funcs, hoisted, … }` refactor (helpers as
`(&mut FunctionBuilder, &mut LowerCtx, …)`) applied across the whole loop before
the first helper moves — an all-or-nothing change to JIT-critical code where a
subtle miscompile passes the tests. Deferred to a focused pass with per-family
JIT-differential + `BROOD_JIT_DUMP_IR` + benchmark verification.

Verified (prepass): compiles default + no-default-features; differential 2/2,
jit 34/34 (under JIT_VERIFY); full suite 3071/3071.

## 2026-07-24 — adversarial pass over `nest`: five real bugs, two of them serious

Wrote a fuzz harness (~50 nasty argument values × every flag and positional, plus
malformed manifests and malformed sources) and looked for hard failures: panics,
hangs, string-interpolation escapes, and internal traces surfaced for user errors.
No panics and no hangs anywhere. **No injection anywhere** either — verified with a
computing oracle (a payload that prints `24690` if evaluated, so an error message
merely *echoing* the payload can't be mistaken for evaluation);
`escape_brood_string` / `blsp_string` hold across every command.

Five genuine bugs, found in roughly increasing order of seriousness:

1. **`nest format` silently RESTRUCTURED code that didn't parse.** The worst of the
   set. The CST the formatter walks is lossless and error-*tolerant*, so it happily
   represents an unclosed list — and its recovery rewrote the file. Given a
   `(defn f …` whose paren was never closed followed by a top-level `(defn g …)`,
   formatting moved `g` **inside** `f` and appended the missing paren at the end.
   Being mid-edit with an unclosed paren is completely routine, and format-on-save
   makes it automatic. `format-file` now gates on the STRICT reader
   (`format--parses?` — the tolerant CST cannot answer this) and returns
   `:unparseable`, leaving the file byte-identical; `format-project` reports
   `skipped (does not parse)`, and `--check` says `does not parse` rather than
   mislabelling it "needs formatting" (which promised a fix that would never come).
2. **`nest add` could brick a project two different ways.** A name that isn't a
   plain symbol was written verbatim into the manifest: `nest add "" :path ../x`
   produced `:dependencies [[ :path "../x"]]`, which no longer parses, so *every*
   later `nest` command failed until the file was hand-repaired. Any name with a
   space, quote, bracket or paren did the same. Now validated by ROUND-TRIP — a name
   is usable iff `read-first` returns the identical symbol, which also rejects a
   numeric name without a hand-maintained character list. Separately, `add` wrote
   the manifest *before* resolving, so `nest add foo :path ../nonexistent` left an
   unresolvable dep behind and broke every later command; the edit is now rolled
   back on failure, making a failed `add` a no-op.
3. **A misspelled manifest head was silently ignored.** `(porject :name …)` — or any
   first form that isn't `(project …)` — was skipped, so every setting in the
   manifest was quietly dropped and the project ran on defaults. It looks like it
   worked, which is the worst failure mode. Now an error naming the file and the
   offending head.
4. **`:source-paths "src"`** (a bare string instead of a vector — easy, since one
   path is the common case) surfaced as `type error: first: expected list or vector`
   raised from inside `map`, with nothing pointing at the manifest. Now names the
   key and shows the fix, with the example matched to the key (suggesting `["src"]`
   for `:test-paths` would be misleading).
5. **An unparseable manifest didn't name the file**, and printed the raw error map.
   Now `project: cannot parse <path> at line L, column C: <message>`.

Also guarded `nest search` outside a project (the last command still leaking
`package--in-project`'s trace).

Manifest and source fuzzing produced **78 and 55 "leak" hits but zero crashes**, so
the data handling was already robust; what the leaks were telling us was a
diagnostics problem, which is what got fixed. 38 new regression tests
(`package_test` 39→55, `project_test` 47→58, `format_test` 56→66).

## 2026-07-24 — Tier 1 item 1 cont.: Op → module scope + extract jit_lower/emit.rs

The enabling refactor for decomposing `jit_lower_arm_inner`'s emit loop. The
blocker was that the `Op` operand-model enum was defined *inside* the function, so
no emit helper could become a free fn. Moved `Op` to module scope in `jit_lower.rs`
(`#[cfg(feature="jit")] pub(super) enum Op`), then extracted the two arithmetic
emitters — `emit_arith` (overflow-checked int arith → deopt) and `emit_float_arith`
(f64) — from their closures (which took `b` and captured the `deopt` block) into
`eval/compile/jit_lower/emit.rs` as free fns `(&mut FunctionBuilder, op, x, y,
deopt)`. Call sites pass `deopt` explicitly.

This proves the pattern for the rest of the emit loop: fn-local types → module
scope, helper closures → free fns in `emit.rs`/(future) `call.rs`/`prim.rs`/
`control.rs` with their captured state (FuncRefs, the `RefCell` slot-tracking) as
params. `jit_lower.rs` 5437 → 4389, now with `i64.rs` (869) + `prepass.rs` (108) +
`emit.rs` (132) split out.

Remaining: the larger helper closures (box_scalar/store_op/as_int/vector_ref/
eq_dispatch/…) and the per-Inst arm bodies, extracted the same way — a continuing
incremental grind with per-step JIT-differential verification.

Verified: compiles default + no-default-features; differential 2/2, jit 34/34
(under JIT_VERIFY); full suite 3108/3108.

## 2026-07-24 — Tier 1 item 1 cont.: extract scalar slot helpers (Frame context)

Continued the emit-loop decomposition. Extracted the scalar slot-access helpers —
`box_scalar` (scalar → (tag,payload)), `load_slot_int` (tag-checked Int load, deopt
otherwise), `store_int` (box + store), `copy_value` (handle-safe whole-Value copy) —
from their closures into `jit_lower/emit.rs`. Their shared captured state (the
`roots` base var `rb_var`, the frame `base`, `nslots`, the `deopt` block, and the
register-carried `carry_vars` table) is bundled into a `#[derive(Clone, Copy)]
Frame<'a>` struct built once after `rb_var` and threaded by value. A `STRIDE`
module const replaces the fn-local one.

One transform gotcha fixed: the `frameize` pass rewrote `{nslots}` inside a
`debug_assert!` format string to `{f.nslots}` (invalid inline-arg syntax) — switched
those to positional `{}` + args.

`jit_lower.rs` 4389 → 4325; `emit.rs` 132 → 237. Verified: compiles default +
no-default-features; differential 2/2, jit 34/34 (JIT_VERIFY); full suite 3108/3108.

## 2026-07-24 — Tier 1 item 1 cont.: extract slot-kind tracking helpers

Continued the emit-loop decomposition: extracted the per-slot type-tracking helpers
— `op_is_float`, `set_slot_float`, `set_slot_bool`, `is_bool_op` — into
`jit_lower/emit.rs`, extending the `Frame` context with the two `RefCell<Vec<bool>>`
slot-flag tables (`slot_float`/`slot_bool`). `op_is_float`'s call sites had nested
parens (`Op::Slot(*slot_a)`), rewritten precisely. jit_lower.rs 4325 → ~4300.
Verified: differential 2/2, jit 34/34 (JIT_VERIFY); full suite 3108/3108.

## 2026-07-25 — jit_lower decomposition: roadmap the remainder + a heavier test pass

Recorded the remaining `jit_lower_arm_inner` emit-loop decomposition steps as a
dependency-ordered checklist at the top of ROADMAP "Active work" (batch 5
operand-materialization → `Funcs` struct → big helpers → per-`Inst` arm bodies),
with a raised testing bar for the work.

Ran that heavier pass against the 4 batches landed so far: differential 2/2 +
jit 34/34 + jit_runtime_compaction 3/3 all green under
`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1 BROOD_JIT_VERIFY=1`; and the full `brood`
Rust test suite (every lib + integration target — `suite.rs`, `runtime_collector`
20/20, preemption/work-stealing/reductions/…) green. Combined with the earlier
`nest test` 3108/3108, the decomposition-so-far is behaviour-identical under stress.

## 2026-07-24 — `nest completions`: project-aware TAB completion

`nest completions <bash|zsh|fish>` emits a shell integration script; TAB then
completes subcommands, flags, **and project-dependent values** — test files, the
`:tags` declared anywhere in the suite (for `--only`/`--exclude`/`--include`),
declared dependency names, module names, and `ValueEnum` choices.

**Split by which side owns the truth.** Subcommands and flag names are read out of
**clap's own argument model** (`Cli::command()`), never a hand-kept list — a flag
added to the `Cmd` enum is completable the same day, and a renamed flag cannot
leave a stale completion behind. Staleness is the failure mode that kills
completions over time, so this was the design driver. Project-dependent values come
from `std/tool/complete.blsp`, and only when the cursor is genuinely at a value
position, so completing a subcommand or a flag never pays interpreter startup
(~60ms static vs ~185ms dynamic in a debug build).

The three emitted scripts are deliberately thin — each forwards the current words
to a hidden `nest complete` and offers what comes back — so there is exactly one
implementation of the logic and the shells cannot disagree with it.

**Two invariants, both tested:**

1. **Completion never fails.** It runs on a keypress, so `nest complete` exits 0
   and writes nothing to stderr whatever it is handed. Verified by a harness over
   ~1300 invocations (every subcommand × ~50 hostile values × three contexts:
   healthy project, unparseable manifest, bare directory): zero non-zero exits,
   zero stderr bytes, zero crash dumps. Also re-ran the computing injection oracle
   (a payload printing `24690` only if evaluated) — never evaluated.
2. **Silence means fall back.** With no useful candidate (`--seed` takes a number),
   nothing is printed and each script defers to the shell's own filename
   completion. A confidently wrong list is worse than none.

Tags are found by scanning test source **text** for `:tags [...]`, not by
registering the suite: loading a project image costs far more than a keypress
allows, and a project whose sources don't compile would complete nothing. A
runtime-computed tag is therefore invisible — fine, since a completion list is a
hint, not a specification.

One new primitive, `(builtin-modules)`, exposes the Rust-side baked-in module table
(a static the language otherwise cannot see); it also lets a module name be checked
before `require`ing it.

**Two bugs caught before shipping**, both by testing rather than reading:
- `std/tool/complete.blsp` failed to load at all, because `project--collect-tests`
  is module-private (ADR-146) and the privacy error fires at **require** time —
  which meant every `complete--safely` net inside was bypassed and completion
  silently produced nothing. Fixed with `(:use-internals project)`.
- The zsh script declared `local -a words`, **shadowing zsh's own
  completion-context `$words`** before it could be read, so every completion would
  have seen an empty command line. Renamed to `parts`. Found by hand-review since
  zsh isn't installed here — bash is verified end-to-end by driving
  `_nest_complete` with real `COMP_WORDS`/`COMP_CWORD`; zsh and fish are
  syntax-reviewed only, which is worth knowing.

`crates/nest/tests/complete.rs` (18 cases, incl. the never-fail matrix and the
zsh-shadowing regression) + `tests/complete_test.blsp` (38 cases over the pure
scanning/keyword/safety-net logic).

## 2026-07-25 — a warning-free test run, and the flaky test I introduced

Two follow-ups to the completion work, both about the suite rather than the feature.

**A test I added was destabilising the suite.** `completion_never_fails_however_it_is_called`
spawned ~300 `nest` subprocesses (each booting an interpreter) and ran 41s — and
under a full `make test` it competed with `brood_suite_passes`, which runs the whole
in-language suite inside one nextest case, for cores and memory. That produced one
intermittent failure of the *wrapper*, not of my test: the suite passed standalone
(3146 cases) and on the next run. Cut the matrix to cover input KINDS rather than
permutations — six distinct shapes (empty, flag-like, path-traversing,
quote/paren, shell metacharacters, interpolation-escape) × three contexts × five
positions — which is 12.8s and no measurable interference. Two consecutive clean
`make test` runs confirmed it.

**The suite now prints no warnings at all.** It had accumulated diagnostics that
were *correct* but expected, which is the worst kind of output: it trains you to
ignore the channel. Fixed at the mechanism level rather than by muting:

- **`*reload-diagnostics*`**, a Brood-visible switch the kernel now consults in
  `def` alongside `BROOD_NO_RELOAD_DIAG`. Coverage instrumentation changes every
  arity by design (10 `[reload] arity changed` lines), and `hot_reload_test` changes
  an arity on purpose. The env var couldn't help — there is no `setenv` primitive and
  the read is cached process-wide — so tests can now scope the diagnostic off. Off by
  default nowhere: unbound means on.
- **`with-err-str`**, the missing counterpart to `with-out-str`, added to the
  prelude. `eprint`/`eprintln` write through the `*err*` port, so this rebinds it to
  a collecting sink (accumulating in a `Table`, the sanctioned mutable structure).
  This is the third time this session I wanted it — the `--failed` warnings, the
  selection warnings, and the `:format-plugins` warning were all previously
  untestable and I said so twice. Now those tests **assert the message** instead of
  merely tolerating it, which is strictly better coverage. It cannot intercept a
  Rust-side `eprintln!`, which is documented on the macro.
- `reload_watch_test`'s watcher announcements (including a deliberately throwing
  callback's `… — boom`) go through `println`, so `with-out-str` captures them —
  and it captures spawned processes, which is where the watcher prints from.
  Assertions throw rather than print, so wrapping a body can't hide a failure.

Also cleared the one clippy warning: the completion section banner had been inserted
between `in_project`'s doc comment and `in_project` itself, orphaning the comment
onto the next function. The doc comment is back where it belongs.

Now: `cargo build --tests` 0 warnings, `cargo clippy --tests` 0 warnings, `nest
check` 0 warnings, and a `nest test` run with an empty warning channel.

## 2026-07-25 — `nest new` produced projects that failed their own toolchain

Nobody had ever scaffolded a project and then run the toolchain over it — the unit
tests cover the template *strings*, not their output. Doing that found two bugs
affecting every user's first five minutes.

**1. Every template shipped code `nest format --check` rejected.** A brand-new
project failed its own CI format gate on the first run, and the starter code
modelled non-canonical style. Three distinct causes: column-aligned manifest keys
(`:name    "x"` — the formatter collapses runs of spaces), comments trailing a form
*inside* a list (re-emitted on their own line, per docs/tooling.md), and
multi-line `defmodule` docstrings sharing the head line.

Fixed systemically rather than by hand-tuning seven templates:
`new-project` now runs every generated `.blsp` through `format-file`. So a NEW
template cannot reintroduce the drift, and a change to the formatter's style is
picked up for free. (`format-file` leaves an unparseable file untouched — the gate
added earlier today — so a malformed template degrades to "written as-is" rather
than mangled.)

That alone made the output *clean* but not *good*: formatting hoisted
`; the app framework …` notes out of their `defmodule` and stranded them above the
whole form, describing the wrong thing. So the templates' trailing clause comments
were moved above their clauses too — the placement CLAUDE.md already recommends.

**2. `nest new --template hatch` promised a next step that cannot work.** Those
templates scaffold `:path` deps on sibling checkouts (`../hatch`,
`../store-postgres`) which are not present — there is no `hatch/` in this workspace,
only `hatch-demo/` — so every command fails on an unresolved dependency. The
scaffolder nonetheless printed `Next: cd x && nest test && nest run`, i.e. the first
thing a user does after `nest new` was guaranteed to fail. It now names the
prerequisite and suggests `nest fetch` once it is in place, with the message
agreeing in number for the single-dep case.

New `crates/nest/tests/scaffold_quality.rs` (7 cases) closes the gap that let this
ship: for each self-contained template it scaffolds and asserts format-clean,
check-clean, passing tests, and no hoisted comment; plus the next-steps accuracy and
that the hatch failure names its missing dep. 3.2s, so it is cheap to keep.

## 2026-07-25 — sweep of the remaining `nest` commands

Worked through the commands the earlier passes hadn't exercised. Most were already
sound; recording what was checked so the next sweep starts further along.

**Verified working end to end** (no changes needed): the whole dependency lifecycle
— `add` → `tree` → lockfile → `fetch` → *using the dep from a test* → `remove`,
with duplicate `add` correctly rejected; `nest run` with `--main`, `--for`, and
trailing args (`--for bogus` gives a clean CLI-level error, `--main main/nosuchfn`
names the fn and module); `nest new .` in-place and `nest new` over an existing
project (both stay format-clean); `nest doc`, `nest tree`; and all three `nest
grammar` targets (the tmlanguage output parses as JSON).

**Fixed: `with-err-str` was missing from the highlight keyword list.** The list
behind `nest grammar` (and the LSP, and the REPL highlighter — `SPECIAL_FORMS` in
`builtins/tooling.rs`) includes `with-out-str` as a highlight-only core macro, so
its brand-new sibling belonged there too; without it editors would colour one and
not the other. Added to `keywords.rs` + the list, and documented in
`docs/language.md` §I/O beside `with-out-str`, with the mechanism difference spelled
out (port-based, so it cannot intercept a kernel-side `eprintln!` — that has its own
switch, `*reload-diagnostics*`). `tests/capture_test.blsp` gained 10 cases for it,
including error propagation and that the sink doesn't leak after a throw.

**Found, not fixed — a design call.** Two projects scaffolded with the default
template cannot depend on each other: both define a `hello` module, so
`nest add` fails with `module name collision — 'hello' is provided by both your
project and dependency 'x'`. The error is clear and names the fix, and the root
cause is that namespaces aren't package-rooted yet (ADR-070). Avoiding it would mean
renaming the demo module after the project, which reshapes the first thing every new
user reads — worth deciding deliberately rather than as a drive-by. (The rollback
added earlier means the failed `add` leaves the manifest untouched, so it is only a
friction, not damage.)

## 2026-07-25 — completion follow-ups: two wrong-value bugs, and real latency numbers

**`nest run --main <TAB>` offered file paths.** `value_kind` had a subcommand-wide
arm — `("check" | "run" | "format", _)` — which also matched `--main`, so every
suggestion for an argument that takes `MODULE[/FN]` was a `.blsp` path. Every arm now
matches the argument NAME as well as the subcommand, and `--main` offers this
project's own source modules (a std module can never be an entry point, so
`complete-project-modules` deliberately excludes them, unlike `complete-modules`
behind `nest doc`).

**`nest new --template <TAB>` completed nothing**, despite the values being a fixed
list. It now reads `*project-templates*` from the scaffolder, so a new template is
completable without touching the completion module.

**Latency, measured in a release build** (the debug figures quoted in the previous
entry were the pessimistic case): **13–15 ms** for a static completion
(subcommands, flags) and **~30 ms** for one that boots the interpreter to read the
project (tags, test files). Both are inside a keypress budget, and the static/dynamic
split is doing its job — the common case is 2× cheaper.

**Also validated on real data:** `--partitions 3` over this repo's own suite splits
1072 / 1047 / 1037, summing to exactly the unsharded 3156. Shards genuinely
partition — nothing dropped, nothing run twice — which is the property a sharded CI
depends on and which the earlier 5-test fixture couldn't really prove.

Two more checks passed with no changes needed: `nest release` bundles and its
standalone binary runs from an unrelated directory, **including dependency sources**
(a project using `(liby-greet)` from a `:path` dep bundles 3 modules and prints
correctly with no project dir present); and the `nest grammar` tmlanguage output is
valid JSON.

Also closed real CLAUDE.md drift: `BROOD_NO_RELOAD_DIAG` was missing from the debug
flag table, along with its in-language equivalent `*reload-diagnostics*`.

## 2026-07-25 — a missing-file inconsistency, and the suite's one un-retried flake

**Fixed: only `nest test` reported a mistyped filename properly.** It said
`nest test: cannot read x.blsp: No such file or directory`; `check` and `run` handed
the path to Brood and surfaced the failure from whichever internal function read it
first — `check-file-deps: cannot read …` plus a trace through
`project--pfold-files` — for what is simply a typo. All three now check at the
`nest` boundary and print the same line. One path deliberately still does NOT get
checked: `nest run <doc>` hands a non-`.blsp` path to the entry point, and opening a
file that doesn't exist yet is the ordinary editor case.
`crates/nest/tests/missing_file.rs` (3 cases) pins both halves.

**Diagnosed a suite flake that was not mine.** `make test` went red on
`brood_suite_passes`; the culprit was `remote-spawn against the local node › a
literal (fn () …) body runs`, which passed 3/3 standalone and 2/2 on immediate full
re-runs. Its `(after 5000 :timeout)` deadline was competing with ~840 parallel
nextest cases for cores. These are *correctness* tests — "did the body run at all" —
so the deadline only exists to stop a hang, never to measure latency; raised to 30s,
with the suite's own 120s per-test ceiling still bounding a genuine hang.

That exposed a **structural gap**: `.config/nextest.toml` already gives
`distribution` and `serve_attach` a retry precisely because real-network timing under
load can fail with no code at fault — but the in-language suite *also* contains
node-talking tests, and because they run inside one wrapper case they were the only
such tests in the workspace with no second chance. One blown deadline reddened all
~3160. `binary(suite)` now gets `retries = 1` too: free on a green run (nextest only
re-runs a failed case), a deterministic regression still fails both attempts, and a
pass-on-retry is reported as FLAKY so it can't be absorbed silently.

Deliberately NOT done: there are 148 `(after …)` deadlines across the suite, and
raising them wholesale would be a large unreviewed change — a small deadline is
correct when the test *asserts* that a receive times out, and only reading each one
distinguishes that from an anti-hang guard. Fixed the observed case; recorded the
pattern so the next one is diagnosed in minutes.

**Fuzz regression check** after the day's changes: the full harness (~50 hostile
values × every flag and positional, malformed manifests, malformed sources, bare
directories) reports 0 panics and 0 hangs, and the computing injection oracle finds 0
evaluations across 5 payloads × 12 argument positions. The 24 "INJECT" flags the
harness prints are its own false positives — a Brood error message echoing the
payload — which the oracle exists to rule out.

## 2026-07-25 — two more leaked-internals fixes, and the last untested commands

**`nest search` printed a raw error structure.** The registry index resolver read
`(or (offload %git-resolve-ref url "main") (error "cannot reach the registry …"))` —
but `%git-resolve-ref` **throws** on an unreachable remote rather than returning nil,
so that fallback could never fire. What reached the terminal was the propagated
`[:error {:kind :runtime, :message …}]` value plus a Brood trace. Now caught, with a
new `package--error-reason` that unwraps whichever shape an error arrived in (string,
error map, or the `[:error <map>]` pair an `offload`ed primitive propagates) into one
sentence naming the URL, the `:registry` setting to check, and the underlying reason
in parentheses. 6 tests.

**`nest observe` / `nest attach` piped gave `os error 6`.** They draw an
alternate-screen TUI, so redirected output failed deep in the render loop —
`runtime error: terminal: No such device or address (os error 6)` with an
`at editor/ui/ui-run` frame. They now check `stdout().is_terminal()` at the boundary
and say so, including the pty recipe (`script -qec … /dev/null`) that CLAUDE.md
already documents for testing TUIs. Verified the pty path still renders and quits on
`q`.

**Smoke-tested the last two untested commands, no changes needed.** `nest mcp` still
completes a real JSON-RPC `initialize` + `tools/list` handshake (20 tools) — worth
confirming after adding two subcommands to the same enum. `nest observe` renders and
exits cleanly under a pty.

That leaves `nest publish` as the only subcommand not exercised end-to-end here: it
writes to a registry index, and the configured one
(`https://github.com/broodlang/registry`) does not exist yet, so there is nothing to
publish against. Its failure path is now legible, which is the part that was broken.

## 2026-07-25 — CLAUDE.md drift audit

Ran the `claudemd-drift` checks, since this session touched several of the surfaces
that file enumerates. Three real deltas, all pre-existing rather than mine:

- **`nest` subcommands** omitted `publish` and `search` — the git-backed registry
  commands from ADR-147 (package manager v2), which landed without the summary
  catching up. Added with their ADR citation. (`completions`/`complete` were added
  earlier today when they shipped.)
- **The `std/tool/*` list** omitted `explain` (the `explain-error`/`find-pattern`
  cookbook, shipped 2026-07-23) and `scaffold` (split out of `project.blsp` in the
  file-organization pass). Added.
- **`process/`** is now described as containing the scheduler, but today's upstream
  refactor moved it into `process/scheduler/` (pool, lifecycle, guards), and the
  one-line summary also predated `links`/`sysmon`/`io_source`. Reworded.

Two sections came back accurate: the **env-flag table** has no dangerous drift —
every flag it documents still exists in the code (the historical failure mode was
`BROOD_TRACE_SAFEPOINT`, documented but never implemented). 31 flags the code reads
are undocumented, which is by design: they are internal JIT/GC tuning knobs, and the
table is explicitly a curated user-facing subset. The **milestone prose** and the
`eval/compile/` file list also matched reality.

## 2026-07-25 — concurrency probe: manifest edits are not safe, and saying so honestly

Ran the `nest` commands concurrently, since an editor plus a terminal is a normal
setup.

**Safe:** four simultaneous `nest test` runs in one project all pass, sharing the
on-disk check cache and the `--failed` record without corruption. Read-only commands
are fine.

**Not safe — a real lost update:** three concurrent `nest add`s all report `added`,
but only some land. Measured across five trials: 2, 2, 3, 1, 2 of three. Each command
reads `project.blsp`, appends its entry and writes back, so the last writer erases the
others' entries. The manifest is never left corrupt (and a *failed* add still rolls
its own edit back), so the damage is a missing dependency, fixed by re-running.

I tried to at least make it loud, by re-reading the manifest after writing and
erroring if our own dep wasn't there. **It doesn't work, and I verified that rather
than assuming it did**: across five trials it never fired once. The loser is whichever
process wrote *first*, and it has already re-read and seen its own entry by the time
the other write lands. So the check was removed — shipping code whose comment claims a
guarantee it doesn't provide is worse than the documented limitation.

Preventing it needs real mutual exclusion: an atomic exclusive-create or an advisory
file lock, neither of which the language has a primitive for. Concurrent manifest
edits are a scripting scenario rather than something done by hand, so that primitive
stays deferred (ADR-011) instead of being invented for this. Recorded as an explicit
one-at-a-time constraint in `docs/packages.md`, with the measurement, why it is
undetectable from inside the command, and what a fix would require.

## 2026-07-25 — Tier 4 io.rs split + jit_lower Batch 5 / Funcs / big helpers

Two tranches of the structural-cleanup + jit_lower decomposition work.

**Tier 4 `builtins/io.rs` split (structural cleanup):** carved the 13-concern
grab-bag down. Crypto+hashing (`HashAlgo`/`hash_algo`/`%digest`/`%hmac`/
`%random-bytes`/`%chacha20-encrypt`/`%chacha20-decrypt`/`%pbkdf2-sha256-bytes`) →
new `builtins/crypto.rs`; the package-manager git/tar mechanism (`run_git`/
`git_or_err`/`%git-resolve-ref`/`%git-changed-files`/`%git-clone`/`%untar-gz`/
`%rm-rf`) → new `builtins/pkg.rs`; and the misfiled transcendental math
(`sin`/`cos`/`tan`/`atan`/`exp`/`asin`/`acos`/`ln`/`log2`/`log10` + `%f64-sqrt`/
`atan2` + the `math1_*` macros) `sequences.rs` → `numeric.rs`. The general byte
helpers `collect_bytes`/`bytes_to_value` stay in `io.rs` (used broadly). Glob
re-export keeps `register()` untouched. io.rs 1932 → 1436; crypto.rs 258, pkg.rs
263. Green: `builtins` unit tests incl. the PRIMITIVE_DOCS drift-guard, and the
crypto/hash/package/format blsp suites.

**jit_lower Batch 5 + `Funcs` + big helpers:** extended `Frame` with
`slot_f64_cache`; moved the operand-materialization family (`read_words`/
`store_words`/`as_int`/`as_block_arg`/`as_f64`) and `store_op` to `emit.rs` as
free fns over `Frame`; added the `Funcs` runtime-call context (heap/out_slot/
ptr_ty/error + vector-slab FuncRefs) threaded alongside `Frame`; and moved the
big helpers `call_handle`/`vector_ref`/`table_prim`/`eq_dispatch` to `emit.rs`
over `(…, Frame, Funcs)`. Every helper keeps a one-line delegating closure at the
original site, so the emit-loop call sites are byte-identical — zero codegen
change. jit_lower.rs 4308 → 3785; emit.rs 273 → 923. Verified per step and at the
end: differential 2/2, jit 34/34 (incl. `GC_STRESS`+`GC_VERIFY`+`JIT_VERIFY`),
full `make test` 811/811 + doctest, and JIT vs `BROOD_VM=0` output bit-identical
across arith/float/vector-ref/keyword-eq. Remaining: the per-`Inst` arm bodies
(the all-or-nothing step) left for a focused, benchmark-gated pass.

The two Tier 4 policy-in-Rust items (`gui.rs` color/UI policy, `nest cmd_run`)
were deferred with rationale — both are outward-facing behavioral changes that
can't be verified without a live display / interactive watch session.

## 2026-07-25 — fixed the manifest race (`%file-swap`)

Yesterday I documented concurrent `nest add`/`remove` losing an update as a known
limitation, on the grounds that preventing it needed mutual exclusion the language
had no primitive for. Asked to fix it, so: added the primitive.

**The shape matters.** The obvious move — a `with-file-lock` builtin wrapping a Brood
thunk — is possible (`apply_value` lets a builtin call back into Brood) but buys two
hazards: a blocking lock around arbitrary Brood code can self-deadlock on re-entry,
and re-entering `eval` from a builtin is a GC-safepoint concern. So instead:
**compare-and-swap**, which needs no callback at all.

`(%file-swap lock-path data-path expected new)` replaces the file's whole contents
only if they still equal `expected`, returning false otherwise. Brood
(`package--edit-manifest!`) reads, computes the edit, and swaps; on a false it
**recomputes the edit against the new content** and retries (bounded at 8, then a
clear "nothing was written, re-run it"). That retry is the whole reason CAS fits: the
modify step is Brood splicing source text, so it cannot run inside a locked
primitive.

Two properties in the primitive, both load-bearing:

- **Serialised** by a blocking exclusive `flock`, held only for the call — it cannot
  leak, and the OS releases it if the process dies, so there is no stale-lock
  recovery to get wrong. The lock is a *separate* file, never the manifest: the
  manifest is replaced by `rename`, and a lock on a since-unlinked inode excludes
  nobody. (I worked that through before writing it — locking the data file plus
  rename is a plausible-looking design that silently doesn't serialise.)
- **Crash-atomic** — temp file + `rename`, so a crash mid-edit leaves the old file
  intact rather than truncated. A half-written manifest is exactly the "project no
  longer parses" failure the day's earlier work was about.

The lock lives in the project's cache dir (keyed by root; `/tmp` without `HOME`), not
the project tree — nothing stray appears beside the user's source, and the inode stays
stable across rewrites. A test asserts that. The failed-add rollback is now a CAS too,
so it can't stomp a concurrent editor; if it can't revert cleanly it says so.

**Measured:** 3 concurrent adds went from 1–3 landing to 3/3 across 8 trials; 6
concurrent adds land 6/6 across 5 trials; mixed concurrent add+remove converges to
exactly the right dependency set every time. `crates/nest/tests/manifest_race.rs` (4
cases, asserting that anything *reporting* success actually landed — a false success
is the bug, a legitimate failure is fine) plus 9 cases pinning the CAS contract in
`tests/file_test.blsp`.

One incidental fix: `package` needed `(:use-internals project)` for
`project--cache-dir` (ADR-146). The privacy error fires at *require* time, so the
whole module silently failed to load until it was granted — the same trap
`std/tool/complete.blsp` hit yesterday.

## 2026-07-25 — jit_lower emit-loop decomposition: the per-`Inst` arm bodies (COMPLETE)

The last, largest step of the `jit_lower_arm_inner` decomposition (ROADMAP "Active
work"). The ~1,600-line emit loop — one big `match &code[j]` over ~15 `Inst` variants,
all sharing `b`, the virtual operand `stack`, the hoist maps, and ~27 `FuncRef`s — is
now split into per-family sibling files, each a behaviour-identical relocation:

- **`jit_lower/control.rs`** — `Jump` / `JumpIfFalse` + the block-param edge-typing
  helper `record_block_flags`. The two terminators emit their branch and return
  `Some(())` (or `None` to bail); the caller keeps the `break`.
- **`jit_lower/prim.rs`** — `Prim1`, `MakeVector`, `Prim3` (table-put), and the fused
  `Prim2` / `Prim2SlotSlot` / `Prim2SlotInt`. `pick` is a free fn here; the big shared
  helpers (`call_handle`/`vector_ref`/`table_prim`/`eq_dispatch`) were already in
  `emit.rs`, and `inline_vec_ref` moved there too so `Prim2SlotInt` can call it.
- **`jit_lower/call.rs`** — the general `Call` (tail/non-tail, the in-IR epoch-guarded
  fast link, handle-spill discipline) and `SelfCall` (the self-tail back-edge with
  carry-var sync, cons-only GC safepoint, checkpoint reset, and BEAM-batched reduction
  poll). `Call` returns a `Flow { Fall, Break }` so tail-vs-non-tail control stays in
  the caller loop; `SelfCall` is always a terminator.

**The enabling refactor** was growing `emit::Funcs` to carry *every* runtime-call
`FuncRef` (car/cdr/cons/makevec*/table*/rb/globic/pushn/callslow/natfl/flbase/fastframe/
sp/tickn, + a `#[cfg(debug_assertions)]` `dbg_staging`) and a shared
`emit::TICK_BATCH`, so the arm fns take just `(&mut b, &mut stack, …, frame, funcs)`.
The operand `stack`, `spill_next`, and `bool_param` thread as explicit `&mut` params.
No codegen change: each closure call (`read_words(&mut b, op)`) became the direct
`emit::read_words(b, op, frame)` it already delegated to. `jit_lower.rs` 3785 → 2271
(5437 → 2271 across the whole decomposition); the trivial leader arms
(`Const`/`Local`/`Global`/`Pop`/`SetLocal`) stay inline, as the roadmap scoped.

**Verified** per family (differential 2/2 + jit 34/34), then the whole split under
`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1 BROOD_JIT_VERIFY=1` (36/36), full `make test`
846/846 + doctest, and a three-engine A/B (JIT vs `BROOD_NO_JIT=1` vs `BROOD_VM=0`) on a
program exercising every arm family — output bit-identical, with `BROOD_JIT_DUMP_IR`
confirming `sum-to`/`fold--loop`/`reverse--acc`/… actually tier through the new code.

## 2026-07-25 — types: sound parameter inference from unconditional demands

The false-positive-clean slice of ROADMAP's "parameter-type inference from body
usage" (deferred under ADR-011 because a naive version guards-false-positives).

`infer_sig` had two tiers: a *precise* one for a body that is a single direct call
(pins each param to the callee's expected type — sound because a straight-line use
is unconditional), and a *return-only* one for everything else (params left `ANY`).
The new middle tier (`collect_param_demands` in `types/check/sigs.rs`) generalises
the precise idea to the **whole body**: it collects a param's type-demand from every
position *guaranteed to execute* on a call — a call argument (including nested), a
`do` form, a `let`-binding RHS/body, an `if`/`when`/`cond`/`case`/`match` **test**,
an `and`/`or` **first** operand — intersecting multiple demands. It descends only
into those positions; a branch/guard arm, an `and`/`or` tail, a `try` body, a nested
`fn`/`quote` are **skipped**, and an inner `let` binder that shadows a param excludes
it within that scope. So `(defn wrap (x) (let (y (+ x 1)) (* y 2)))` now infers
`x : number` (the `let`-RHS always runs), while `(defn f (x) (if (number? x) (+ x 1)
x))` leaves `x` unconstrained (guarded) — the guarded-use false positive can't arise.
Return inference is untouched (params still `ANY` there), so every return-pinned test
is byte-identical.

**Companion fix (`global_value_ty`):** a `*earmuffed*` global now types as *unknown*,
not by its load-time default. `*project-root*` is `(def *project-root* nil)` reassigned
to the real path at runtime — a redefinable/dynamic-by-convention global, which the
type philosophy makes `dynamic()`. Pinning it to `nil` was a pre-existing imprecision
(it produced a baseline `(canonicalize *project-root*)` warning); typing it unknown
clears that, and prevents the new param tier from surfacing the same class at
`(path-join *project-root* rel)` (which is even guarded by a preceding `nil?` throw).

**Verified.** 413 checker tests (added guards for nested-call / `do` / unknown-callee
demands, and for guarded-`if` / `or`-tail / `when` / `try` / shadowing *not* warning,
plus earmuffed-vs-plain global typing); the one outdated test that asserted a `let`-RHS
demand was "unsound" was corrected (a `let`-RHS is unconditional — the warning is a true
positive). `nest check` clean on `tests/`. A whole-`std/` before/after `--check` sweep
(baseline built from `main` in a throwaway worktree): argument-type warnings went 6 → 5
— the param tier **added zero** and the earmuff fix **removed one** pre-existing false
positive. Full `make test` green.

## 2026-07-25 — framed reads: the input-side twin of iolists (tcp-read-until / -n)

ROADMAP's "growable read buffer." The original framing — "a transient/builder value
+ freeze" — was rejected on contact with ADR-026: a user-facing mutable/transient
buffer is exactly the thing immutability forbids. And the O(n²) it was meant to cure
is already gone — iolists (ADR-139) plus the cons-accumulate `tcp-drain` idiom make a
drain O(n). What the five sites (body drain, head reader, chunked de-chunk, WS
reassembly, live-view render) actually repeated was the *receive → accumulate → split*
loop, not a buffer type.

So the fix is **combinators**, in Brood (`std/net/tcp.blsp`):

- `(tcp-read-until sock sep)` → `[head rest]` — read `[:tcp sock data]` chunks until
  the byte sequence `sep` (string or `bytes`) first appears; `head` is everything up
  to AND INCLUDING it, `rest` the surplus already read past it. `[:closed acc]` if the
  socket closes first. For a delimited frame (HTTP head `\r\n\r\n`, a line).
- `(tcp-read-n sock n)` → `[data rest]` — read until at least `n` bytes have arrived;
  `data` is exactly the first `n`, `rest` the surplus. `[:closed acc]` on early EOF.
  For a length-prefixed body (Content-Length, a chunk, a WS payload). Tracks a running
  byte count, so it never rescans — O(total).

Both are pure `receive` loops over an immutable reversed-chunk accumulator with a
single `bytes-concat` at the end (no per-chunk rebuild) — no mutable value anywhere
(ADR-026). They return the leftover so a protocol reads frame after frame off one
stream. Built on `bytes-index-of`/`subbytes`/`count`. Binary mode recommended
(`tcp-set-binary`) so lengths/delimiters are byte-exact.

A caller-managed *exposed* accumulator value (for the interleaved WS-gather case, where
the receive loop isn't a simple drain) is deferred (ADR-011) — no in-repo consumer, and
the combinators cover the socket cases.

Tests: `tests/tcp_test.blsp` — 6 real-loopback cases (delimiter frame, delimiter split
across sends, delimiter-never-arrives `[:closed]`, exact-n + surplus, multi-chunk n,
short-EOF `[:closed]`). `nest check` clean; the tcp suite is 15/15.

## 2026-07-25 — `nest test` output: dots by default, an informative coloured trace, and `:skip`

Three connected changes to what a run looks like.

**Tracing is now opt-in.** `nest test` used to pass `:trace` always, so every run
printed `▶ group › name` per test — fine for finding a hung case, noise for everything
else. The default is now **one character per finished test** (green `.`, red `F`,
orange `○`), printed as results arrive from the driver, so a long suite shows movement
and a failure is visible immediately. `--no-trace` is gone; `--trace` opts in.

**The trace reports outcomes, not starts.** Colouring by outcome is impossible from a
start-of-test line — the result isn't known yet — so `trace-result` replaces
`trace-start` and prints when the test finishes, with the duration and the declaration
site (`file:line`, cwd-relative; the loader records absolute paths and a full
`/home/…` on every line buries the part that matters):

```
✓ math › adds                     2ms  tests/math_test.blsp:12
✗ math › divides                  0ms  tests/math_test.blsp:18  (1 failure)
○ db › needs postgres         skipped  tests/db_test.blsp:5
```

Losing the start-of-test line costs nothing for hang-hunting: the runner already
hard-kills at `*test-timeout-ms*` and reports a timed-out failure, so a hung test
shows up as a red line rather than as a start with no end.

**`:skip` is new** — "orange for skip" needed something to colour. `:skip` on a `test`
or a `describe` registers the case but never calls its body, so it is still counted
and still reported: that is the whole difference between skipping a test and deleting
or commenting it out. It composes with `:serial`/`:isolated`/`:tags` in any order, and
counts as neither pass nor failure (`8 tests, 5 passed, 0 failed, 3 skipped`).

Implementation notes worth keeping:

- The result tuple grew a 5th (skipped) and 6th (declaration site) element. `r-skipped?`
  and `r-where` read them **defensively**, because results synthesised elsewhere — a
  dead worker, a timed-out unit — are still 4-element tuples and must not index past
  the end. A test pins that.
- **Colour is real now.** `docs/testing.md` had claimed for a while that output is
  "coloured only when stdout is an interactive terminal (via the `stdout-tty?`
  primitive)" — but `test.blsp` contained no colour code at all. It does now, behind
  exactly that gate, so a piped run, a CI log, or an LLM reading the output still gets
  plain text. The claim and the code finally agree.
- The summary line is built with one `str` rather than `println`'s space-joined
  arguments, so the comma before the optional `N skipped` clause lands correctly.

`tests/runner_progress_test.blsp` (20 cases) covers the marks, the three trace
outcomes, the counting, and the defensive accessors.

## 2026-07-25 — making a `nest test` failure readable

Asked whether the output makes sense, and it didn't — the part you act on was the
hardest part to find. Four fixes.

**The anchor was buried in an absolute path.** Every failure opened with
`/home/you/projects/thing/tests/x_test.blsp:5:28:` — the location is the whole point of
the line, and it was 60 characters of prefix. Now relative to the working directory,
which keeps it short *and* still clickable (compilation-mode resolves against cwd, and
`nest test` runs from the project root). Bold, so it is findable in a wall of output;
`test failed:` in red; the labels of the detail lines dimmed so the values read as the
content. Blank line between blocks, which previously ran together.

**Some failures had no location at all.** `fail-loc` is per-assertion, and an assertion
whose form carried no recorded position produced just `test failed: group › name` —
nothing to jump to. It now falls back to the **test's own declaration site**, which the
result already carries (added for `FILE:LINE` selection).

**A test file that fails to load took the whole run down**, and printed the reason as a
structured map:

```
1337:13: error: {:trace ({:col 58, :line 1093} …), :message unbound symbol: …,
                 :file /home/…/tests/oops_test.blsp, :kind :unbound, …}
```

Three thousand passing tests discarded, and the three facts that mattered (file, line,
what was wrong) buried in a printed map — for the everyday case of editing a test file
that doesn't compile yet. It is now one ordinary located failure, the other files still
run, and the run still exits non-zero:

```
tests/oops_test.blsp:2:1: test failed: tests/oops_test.blsp › failed to load
    cannot load: unbound symbol: this-is-not-defined
```

**A failing suite appended a Brood stack trace to its own report.**
`run-project-tests` raises `N test(s) failed` so `cargo test` sees a non-zero exit —
correct, but `nest test` then reported that raise as an error, tacking
`1337:13: error: 2 test(s) failed`, `at project/run-project-tests` and a version banner
onto the end of a clean report. A failing suite is an expected outcome, not an internal
error: `run_expecting_failure_signal` exits non-zero silently for that specific signal
and still reports anything else (a broken manifest, an unloadable file — both verified).

Also worth recording, since it cost real confusion: **`nest test` in this repo showed
12 failures from the installed binary while a freshly built one showed none.** Not a
regression — `std/` is baked into the binary at compile time, so an installed `nest`
one commit behind was running the *old* test framework against the *new* test files,
and every case exercising `:skip`/`progress-mark` failed. `make install` after changing
`std/` is not optional.

`tests/runner_progress_test.blsp` grew to 32 cases, covering the load-failure result,
the anchor fallbacks, and the relative-path rendering.

## 2026-07-25 — clearing the known-issues list: three fixed, one reframed, one honestly deferred

### `nest add`/`fetch`/`tree`/`remove`/`update` ignored the configured registry

Not just untested — **broken**. Only `publish` and `search` bootstrapped with
`(project/load-config)`; the other five didn't, so `*config-registry*` stayed at the
hardcoded default and `nest add pkg :version 1.0.0` failed against a perfectly good
registry. They now share one `PACKAGE_BOOTSTRAP` constant so they cannot drift apart
again.

This also retires "`nest publish` is untestable". It is fully testable: the index can
be a **local directory**, the package source a **local git repo**, and `:registry` is a
user-config key that honours `XDG_CONFIG_HOME` — so `crates/nest/tests/registry.rs`
(6 cases) drives publish → search → `add :version` → *calling the dependency* without
touching the developer's real config. Writing the test is what found the bug.

### 253 `(after …)` deadlines, audited

Classified rather than blanket-raised: a short deadline is *correct* when a test
asserts that nothing arrives, and only the surrounding assertion distinguishes that
from an anti-hang guard. The discriminator turned out to be whether the sentinel is
*compared against* — `(assert= :none (receive … (after 2000 :none)))` asserts a
timeout, while `(assert= 7 (receive … (after 5000 :timeout)))` guards against a hang.
114 guards vs 125 timeout-assertions.

The 100 guards whose sentinel names a *failure* (`:timeout`, `:NEVER-RAN`, `:lost`, …)
in the 1000–10000ms band now use a named `*test-wait-ms*` (20s) instead of a literal.
Naming it is the point: the intent ("this is an anti-hang guard, not a latency
budget") is now in the source, so the next reader doesn't have to re-derive the
classification I just did. Deadlines ≤ 500ms were left alone — those are polls and
drains, and stretching them would change what they test. `process_limit_test`'s 30s
guards are deliberately *above* the knob (they build large structures under a memory
cap) with a comment saying so.

### Two scaffolded projects can now depend on each other

`nest new` gave every project a module called `hello` *and* a `main`, so adding one as
a dependency of another failed on a module-name collision — the command that creates
every project produced projects unusable as dependencies. Two fixes:

- The library module is named **after the package** (`nest new greeter` →
  `src/greeter.blsp` providing `greeter`), which is collision-free and the more
  idiomatic shape anyway. The name is sanitised to a safe symbol, since
  `project--valid-name?` permits punctuation a symbol shouldn't carry (`my.app` →
  `my-app`, a leading digit gets `p-`, pure punctuation degrades to `lib`, and the
  reserved `main` becomes `main-lib`).
- A dependency's **`main` is exempt** from the collision check. It is an entry point,
  not library surface: `nest run` only ever uses the root project's `:main`, and
  nothing requires a dep's.

### The unidentified suite flake: evidence, not a fix

Hunted it with repeated suite runs under 6-core saturating load. **Two of my three
attempts were invalidated by my own contamination** — round 1 rebuilt the binary
mid-experiment, round 2 froze the binary but not the test corpus I was still editing
(the "13 failures" were exactly my 13 new tests against a binary predating them).
Round 3 used a fully isolated snapshot: its own binary *and* its own `tests/`.

Result: **zero intermittent failures across 6 runs**, plus one deterministic failure
that was itself a snapshot artefact (`format: the prelude is idempotent` slurps
`std/prelude.blsp` relative to cwd, and the snapshot had no `std/`).

Round 1's one failing run is the useful datum: every failure in it was a monitor
`[:down …]` receive or a cross-process face lookup — all 5s-deadline guards, i.e.
exactly the class the `*test-wait-ms*` work above addresses. So the evidence says the
flake was that class and is now fixed. I can't prove identity, and won't claim it.

### Line coverage: attempted, reverted, and now better understood

ADR-148 predicted the cost as "a compile-time instrumentation seam". I tried to dodge
that with a cheaper one — record `(file, line)` from the tree-walking evaluator, where
`Heap::form_pos` already carries both, and have `--cover-lines` select `BROOD_VM=0`.
It recorded top-level forms correctly and **nothing inside a function body**, because
a compiled closure's body executes in `exec_value`/`exec_chunk`, not in `eval` — and
`exec_value` has no arm in scope, so no `src_file` to attribute a line to. Threading
it through a hot recursive function and every call site is precisely the high-risk
change the cheap seam was meant to avoid.

So the whole attempt is reverted (the function tier is untouched and verified). The
finding is worth more than the code was: **the tree-walker is not a viable seam for
line coverage**, which rules out the cheap option ADR-148 didn't consider and leaves
compile-time instrumentation of the bytecode as the real path.

## 2026-07-25 — line coverage, third attempt: the denominator was the whole problem

`nest test --cover-lines` now works (ADR-148 tier 2, [`coverage.md`](coverage.md)).
The recording side was straightforward once the previous entry's finding was accepted:
`Inst::RecordLine(u32)`, emitted by `emit_node` only when `BROOD_COVERAGE` is armed,
executed by `exec_chunk` — which already holds the arm's `src_file`, so no new state
threads through the hot executor. An unarmed run's bytecode is byte-for-byte unchanged.
Verified in one line: with the flag set, calling `(lcov/covered 1)` recorded
`["lcov.blsp" (6)]` — the *body's* line, which the reverted `eval` hook could never see.

Then three attempts at the number, two of which produced a confidently wrong one:

1. **Denominator = "lines that hold a form"** (re-read the source, walk the read forms).
   A fixture whose every function ran reported **14%**. A `defmodule` header, a
   docstring and a `defn`'s own line all hold forms and none is an instrumented node —
   the two halves of the ratio described different populations.
2. **Denominator = lines actually instrumented**, registered by `note_instrumented` as
   `emit_node` emits them. Same fixture: **100%**, with a deliberately-uncalled
   `never-run` in it. Arms compile on first **call** (`compiled_arm_for`), so a dead
   function was absent from both halves and the ratio was a tautology.
3. **Force the compile.** `%coverage-precompile` compiles a function's body without
   calling it; `coverage-line-begin!` runs it over every project function before the
   suite. The fixture now reports 17%, rises to 50% when a test calls the dead
   function, and 100% when everything is called.

The general lesson is worth more than the mechanism: **a wrong percentage is worse than
no percentage** — it is exactly the number people put in a CI gate, and both wrong
versions looked plausible in the report. `crates/nest/tests/coverage_lines.rs` pins
against both failure modes by construction (a fixture with one live and one dead
function must report strictly between 0 and 100, and must *move* when a test is added).

### A real bug found on the way: std modules were attributed to the requiring file

The tier-2 report initially credited `std/log`'s lines (127–131, 150–152, 175 …) to a
21-line `src/main.blsp` — which is how the bug was noticed at all. Cause:
`%load-string` — how baked-in std modules load — set no current file, so their forms
inherited whatever file was mid-load when the `require` ran. Not a coverage-only
artefact: the field is `CompiledArm::src_file`, which also names the file in `:trace`
frames. (I could not construct a std-module trace frame on demand to show the wrong
name directly, so that half is inference from the shared field, not an observation.)
`%load-string` now takes an optional name and `require--force` passes
`<std>/<key>.blsp` — honest that there is no openable path, and no longer someone
else's name. `source-location` was never affected; definition sites are recorded
separately.

### Smaller things

- A `--cover-min` shortfall now prints `FAILED: coverage N% is below the required
  minimum M%` and raises a bare signal `nest` recognises, instead of appending
  `1:58: error: …`, `at error` and a version banner to a report already read. Same
  treatment a failing suite has had; `run_expecting_failure_signal` takes a signal
  *list* now.
- A file with nothing instrumented (every function literal-bodied) is omitted rather
  than reported as 0% — a 0% there would fail a `--cover-min` gate for having nothing
  measurable.
- Known under-count, documented rather than hidden: a nested `(fn …)` inside a body
  compiles when the enclosing body runs, so an unexecuted body's inner closure stays
  unmeasured. It errs toward reporting *less* coverage.
- The previous entry's flake hunt closed out: 7/7 isolated runs, the single failure
  identical in all 7 (so deterministic, not a flake) and confirmed a snapshot artefact —
  `format: the prelude is idempotent` passes against the live tree.

## 2026-07-25 — KI-10 (the `receive` 13-arm cliff) no longer reproduces

The last open entry in [known-issues.md](known-issues.md). Bisected 2026-07-11: a
single trivial 13th arm on `buffer--serve` took the buffer suite from 4.9 s / 139 MB
to 8.0 s / 248 MB, worked around by merging two `[:edit …]` arms to stay at 12.

Re-measured on `b0b4fd1` in the configuration it was seen in — the module **baked into**
a `cargo build --release` binary, not hot-loaded:

| serve arms | buffer suite | full suite |
| --- | --- | --- |
| 12 (committed) | 3.33 s / 55 MB | 22.49 s / 610 MB |
| 13 | 3.35 s / 54 MB | — |
| 20 | 3.36 s / 55 MB | 22.79 s / 636 MB |

+1.3% wall / +4% peak for **eight** extra arms, against +65% / +80% for one. Gone.

Two things worth keeping from the hunt. First, **arm count alone was never the
trigger**: a 13-arm receive of uniform trivial arms (200k messages through a spawned
loop) showed no cliff then and shows none now, which is why the original report is
specifically about `buffer--serve`'s heterogeneous arms. Second, **I never identified
the mechanism**, so this is an incidental fix from the VM/JIT/pattern work of the last
fortnight, not a targeted one — recorded that way rather than claimed as a fix. The
arm-count budget in `buffer--serve` is lifted; the merged `[:edit …]` arm stays because
one arm for both shapes is simpler, not because it has to.

A methodological note, since two hot-load A/Bs nearly sent me the wrong way: the
release-fast binary (`--no-default-features`) has no `test` module, and hot-loading a
modified `std/` copy over the baked-in one is *not* the same configuration as baking it
(different region, different freeze path). Both A/Bs happened to agree here, but the
baked build is the one that answers the question.

### Follow-up: the std attribution fix, done properly

The first cut named a baked-in module `<std>/log.blsp` — honest, but not openable, which
makes it useless to any tool handed the path. The real defect was that the embedded
module table (`CORE_MODULES` / `DEV_MODULES`) threw away the one thing that answers the
question: each entry is an `include_str!` of a path it then forgot.

Entries are now built by an `embedded_module!("log", "std/log.blsp")` macro producing an
`EmbeddedModule { key, source, path }`, where `source` is `include_str!` of `path` — so
the recorded path cannot drift from the file the source came from. A new
`%builtin-module-file` reads it back, and `require--force` hands it to `%load-string`.
`std/log`'s forms now record as `std/log.blsp`, and a bundled module (ADR-038, genuinely
pathless) gets `<bundle>/<name>.blsp` rather than a fabricated one.

The regression test is `crates/cli/tests/std_attribution.rs`, and the property it uses is
worth stealing: **every recorded line must exist inside the file it is attributed to.**
No knowledge of what the lines contain, no brittle expected-value list — a line borrowed
from another file almost always lands past the end of the file it was credited to, which
is exactly how the bug showed itself (a 21-line `main.blsp` credited with line 175).
Verified to fail with the fix reverted and pass with it restored, which is the only way
to know a regression test tests anything. It also probes bodies via
`%coverage-precompile` rather than by calling them: arms register their lines when they
compile, and precompiling is both cheaper than arranging real calls into `log` and the
exact path the bug was found on.

## 2026-07-25 — the sibling projects: two fully broken, both fixed; one left to decide

Swept `nest test` across the 14 sibling projects in `~/src/broodlang`. Eleven were green
(brood-edit 829, hatch 750, pong 103, todo 37, willem 21, store-postgres 44, hatch-demo 89,
pong-sound 92, mylife 13, mitch 2, brood-benchmark 2). Three were not.

**brood-chat and brood-terminal didn't run at all** — both died at load on
`lineedit--init` being module-private (ADR-146). The privacy error was a red herring: that
name doesn't exist any more. ADR-146's own commit (`4d9056b`) *promoted* it to the public
`lineedit-init`, and neither downstream project was updated. Same for `lineedit--handle` →
`lineedit-handle` and `lineedit--remember` → `lineedit-remember`.

`lineedit--display-prompt` was the interesting one: still private, and needed by **both**
projects for the same reason — each draws its own input line, so neither can otherwise know
whether to show the configured prompt or the reverse-i-search one. Two independent
consumers needing a `--` helper is the signal that it belongs in the public API (exactly
the argument that made `lineedit-init` public), so it is now `lineedit-display-prompt`,
with four cases in `tests/lineedit_test.blsp` pinning the contract they rely on.

Downstream, after those renames: brood-chat 102/102, brood-terminal 51/51, both
`nest check`-clean. brood-chat also still called the prelude's `ensure-link`, superseded by
`net/reconnect/watch` on 2026-07-18 — updated (a runtime dial path its tests don't reach,
so the evidence there is the checker warning clearing, not a passing test).

**brood-life's 5 failures need a decision, not a fix.** Its `bitboard-bs` board — the
refc-shared bitset twin of the bignum `bitboard`, opt-in behind `LIFE_BITSET=1` — is built
on `bitset`/`bitset-set`/`bitset-count`/`bitset-positions`/`bitset-and`/`bitset-xor`/
`bitset-life-step`, and that whole kernel API (plus `Value::Bitset`) was deleted on
2026-06-28. Notably the deletion arrived swept into a *REPL* commit (`0b3c392`, "Also
in-tree (parallel kernel work, green)") with no ADR and no devlog line, so whether it was
a deliberate POC cleanup or collateral is not recorded anywhere. Already logged as
pre-existing on 2026-07-03. Either the dead variant goes, or the kernel API comes back —
the latter being a `Value` kind plus a fused CA primitive, i.e. its own ADR. Not decided
unilaterally.

Method note: my first sweep for stale private names deduped with `sort -u -t: -k3`, which
keys on the *name* — so brood-terminal's three call sites vanished behind brood-chat's
identical ones and I "fixed" one file thinking it was the only one. Dedupe by file+name or
not at all.

## 2026-07-25 — external conformance corpora, suite 1: `parse-number-fxx`

Every test in this repo was hand-written, with one partial exception
(`numeric_conformance_test.blsp`, whose cases were adapted *by hand* from chibi's
r7rs-tests and the Gabriel suites). So the correctness bar was "cases we thought
of". Started closing that with the corpora other implementers already paid for in
production bugs. The inventory of 16 suites plus the EMI technique is now a table
in ROADMAP ("External conformance corpora"); this entry is suite 1.

**Infrastructure.** `scripts/fetch-corpus.sh` fetches into `tests/corpus/<suite>/`,
`tests/support/corpus.blsp` locates and reads them (on `:source-paths` so runners
can `(:use corpus)`), runners are `tests/conformance_<suite>_test.blsp` tagged
`:conformance`. Subsampling is "every Nth line", never random, so re-running
reproduces the committed bytes. `data/` is the committed subset; `--full` pulls the
real upstream into a gitignored `full/`, which `corpus-files` prefers when present —
so the same test is a CI gate and an exhaustive local sweep with no code change.
Each suite's README pins upstream URL, commit and licence (no GPL data: `ansi-test`
gets mined for ideas, not vendored). Documented in `docs/testing.md`.

**parse-number-fxx** (nigeltao, Apache-2.0): 33,552 cases vendored out of ~270 MB
upstream — the curated small extractions whole, every 16th line of the two mid-size
ones. All pass, 604 ms in release. Zero findings, which is the expected result for
a parser that delegates to Rust's `f64::from_str`; the value is the *regression
gate*, and it covers the int/bigint→f64 path too, since `string->number` reads a
digits-only input as an Int.

**Needed one new kernel primitive, `float->bits` / `bits->float`** — bit
reinterpretation of a binary64. Not expressible over the existing primitives (no
bitcast, no `frexp`), and it is the only *exact* float comparison there is: `=`
collapses `-0.0` with `0.0` and calls every NaN unequal, so asserting a parse
against `=` would silently accept a wrong sign of zero. General capability, not a
test hook — serialization and float hashing want it too.

**Fell out: a real bug in `project-setup`.** It applies the manifest only
`(when (file-exists? mf))`, and `project-apply` assigns `*project-source-paths*`
only `(when src)` — so a root with no manifest, or one that omits `:source-paths`,
silently inherited the *previous* project's paths. Latent until adding
`:source-paths ["tests/support"]` to this repo's manifest made two
`project_test.blsp` cases fail. Now reset to the `src`/`tests` conventions at the
top of `project-setup`, the same "always reset stale state" discipline the
`*format-headers-extra*` line below it already had. Matters for the long-lived
tooling images (the LSP and `nest mcp` set up a second project in the same image).

Suite green: 3266 in-language tests, `nest check` clean.

## 2026-07-25 — conformance corpora, suite 2: `dectest` finds two real decimal bugs

Wired Cowlishaw/IBM's General Decimal Arithmetic Testcases (ICU licence, fetched from
CPython's verbatim copy). Unlike `parse-number`, this one found bugs.

**Scope had to be drawn honestly first.** dectest specifies IEEE 754 *decimal* — a
context with precision, rounding mode and exponent range. Brood's `Decimal` is
arbitrary-precision and exact, with no context. So the runner keeps only cases where
the two models must agree: finite operands, and a reference result with **no
condition flags**, meaning the context didn't round/clamp/overflow and dectest's
answer *is* the exact answer. `Inexact`/`Rounded`/`Subnormal`, NaN/sNaN/Infinity and
all of `divide` are skipped, each exclusion named in the runner header. ~1,900 of the
5,616 vendored lines apply. What survives is the valuable part: the **ideal-exponent**
rules.

**Two bugs, both `bigdecimal` identity short-circuits Brood inherited.** Its `Sub`
returns the other operand untouched when one side is zero, so `1 - 0.0` gave `1`
instead of `1.0`; its `Mul` does the same when one side is *one-valued*, so
`1.00 * -1` gave `-1` instead of `-1.00`. Each discards the short-circuited side's
scale. `Add` short-circuits neither — so `+` and `-` disagreed with each other, which
is what made it obvious this was arithmetic and not just scale-insensitive printing.
Significance surviving arithmetic is the entire reason to use a decimal for money:
`(* price 1.00M)` silently dropping to whole units is a live-fire bug.

Fixed in `num_bin`: the exact-decimal path now takes a `dec_scale` rule alongside
`dec_op` (`max(sa,sb)` for `+`/`-`, `sa+sb` for `*`) and pins the result with
`with_scale`. The exact result never needs *more* than the ideal scale, so this only
ever pads with zeros — it cannot round. Threaded as an explicit fn parameter rather
than branching on the op name string.

One deviation remains and is pinned as such: a zero *result* prints scale-less
(`1.50M - 1.50M` renders `0`, dectest says `0.00`). That is `bigdecimal`'s Display,
not the arithmetic — the scale underneath is now right — and it is consistent with
Brood's `=`, which already ignores scale (`1.5M = 1.50M`). The sweep can't see it
(both sides canonicalise), so it gets its own test.

Suite green: 3275 in-language tests, Rust nextest clean, `nest check` clean.

## 2026-07-25 — conformance corpora, suite 3: JSONTestSuite finds an RFC bug and KI-11

Wired nst/JSONTestSuite (MIT) — 318 documents whose *filenames* are the RFC 8259
verdict: `y_` must parse, `n_` must be rejected, `i_` is implementation-defined.
Vendored `test_parsing/` only (1.6 MB of a ~60 MB repo that is mostly parser
binaries). A file that isn't valid UTF-8 raises in `slurp` before `json-parse` sees
it, which is the correct verdict, so the runner counts unreadable as rejected.

**`std/json` violated RFC 8259 §7**: it accepted unescaped control characters inside
strings, so a raw tab or newline in a string body parsed as content where every
conforming parser rejects it. Three `n_` cases caught it; one clause in
`json--string--acc` fixes it.

**KI-11 — the real find, and the first open bug in the tree since 2026-07-19.** Two
documents didn't fail a test, they *aborted the OS process*:
`n_structure_100000_opening_arrays.json` and `n_structure_open_array_object.json`.
`thread '<unknown>' has overflowed its stack / fatal runtime error`. An abort, not a
panic — so `install_crash_dump` never fires and there's no artifact to read.

The three-engine oracle made the diagnosis immediate:

| engine | 20,000-deep nested JSON |
|--------|-------------------------|
| default (JIT) | aborts |
| `BROOD_NO_JIT=1` | parses fine |
| `BROOD_VM=0` | clean catchable `recursion too deep … over the 12582912-byte budget` |

`gdb --batch -ex run -ex bt` puts it in `jit_runtime::jit_run_fast_link` ←
`brood_rt_fast_frame`: the JIT's fast-link Brood→Brood call path takes a native frame
per call with **no depth guard**, so it never reaches the VM's `MAX_BC_FRAMES` cap or
the tree-walker's byte budget. Threshold is between 10k and 20k levels. Note a plain
`(defn deep (n) (+ 1 (deep (- n 1))))` runs to 200,000 fine — the repro needs a real
workload hot enough to tier the recursive arms, which is presumably why the existing
recursion-depth tests never saw it.

Impact: any service parsing untrusted nested input is killable with a few kilobytes,
`try`/`catch` can't see it, and a supervisor can't restart from it because the OS
process dies rather than the green process. Filed as KI-11 with a repro and a fix
direction (a remaining-native-stack probe that bails to the VM, or a counter raising
the same catchable error — `stacker` is already a dep and does exactly this for the
checker's deep-CODE walkers). **Not fixed** — JIT call-path surgery is its own change,
not something to fold into wiring up a corpus. The two documents are excluded by name
in the runner, citing KI-11, and the exclusion goes when the bug does.

Suite green: 3284 in-language tests, `nest check` clean. (One run showed two tcp
idle-timeout failures at 53 s wall vs the usual 24 s — load-induced timing flake in
those tests; clean on re-run.)

### The error message that made the sweep hard

Both broken projects died on `lineedit--init` being "module-private" — and that was not
what was wrong: the name had been *promoted* to `lineedit-init` and no longer existed at
all. The message sent the reader looking for a `(:use-internals …)` grant that would not
have helped.

`enforce_private_refs` now checks the global table before wording the error. When the
`--` name is absent from the module AND the single-dash spelling is present, it says so
and names the replacement:

```
`editor/lineedit/lineedit--init` does not exist in `editor/lineedit` — it looks like
a `--` helper that was promoted to the public `editor/lineedit/lineedit-init`. Use
that name.
```

Both halves must be confirmed before it claims a rename: a module that hasn't been
loaded has neither name, so it falls through to the plain privacy message rather than
guessing. `tests/private_test.blsp` covers all three paths — promoted, genuinely private,
and neither.

## 2026-07-25 — syntax finalisation: seven places the surface reinterpreted instead of rejecting

A review of the *language surface* (asked for as "review the language itself", then
narrowed to "we need to finalize the syntax") found one recurring failure mode
rather than seven unrelated warts: **the grammar accepted two spellings for the
same slot, so a wrong guess produced a different working program instead of an
error.** Every item below is that shape. Recorded as ADR-149/150/151/152.

Confirmed by probing the release binary, not by reading — each one reproduced:

| written | was | now |
|---|---|---|
| `(defn g ([x] :one) ([x y] :two))` (Clojure multi-arity) | one 2-param fn, patterns `[x]`/`:one`, empty body → misleading arity error at the call site | error + hint (ADR-149) |
| `(let [[a 1] [b 2]] …)` | destructured `[a 1]` against `[b 2]` → `unbound symbol: b`, no hint | error + hint |
| `(try … (catch Exception e (println "caught" e)))` | bound `Exception`, evaluated the prelude's `e` → printed **2.718…** | error + hint (ADR-152) |
| `(defn f ((x) …) ((x &optional (y 5)) …))` | `&optional` matched as a literal symbol → `[:match-error …]` | error naming the two axes |
| `(defmodule m (:use-internal json))` (typo) | silently ignored — no import, no privacy grant | error |
| `` `(a `(b ~(+ 1 2))) `` | `(a (quasiquote (b 3)))` — inner unquote expanded at the outer level | error + hint |
| module `a` and `b` both `(def *width* …)` | one shared root binding; `b`'s load clobbered `a`'s, `(a/a-width)` → 999 | `a/*width*` and `b/*width*` (ADR-151) |

Two more came out of the same pass:

- **Pins moved from `~x` to `^x`** (ADR-150). A pin *was* `(unquote x)`, so a macro
  template consumed it and could not emit a pinned pattern at all — exactly what
  wrapping `(receive ([:reply ^tag v] …))` needs. 167 pins migrated mechanically
  with a scanner that tracks quasiquote context, so only real pins were rewritten;
  `~` now belongs to quasiquote alone.
- **Arity precedence** was order-dependent: `((x) :one)` vs `((x &optional y) …)`
  tied on `(no-rest, nrequired)` and `max_by_key`'s last-wins picked the
  `&optional` arm for `(f 1)`, contradicting the documented "exact fixed arity
  beats a variadic one". Added a fewest-optionals tie-break to **both** engines'
  selectors (`Closure::select_arm`, `CompiledClosure::arm_for`).

What the new checks found on their own, which is the argument for having them:
`(:doc "…")` in a `defmodule` header — used by `encoding`, `datetime`, `stats`,
`stream` — is not a clause and never was, so all four modules had been **dropping
their module docstring** silently since it was written. `(module-doc 'encoding)`
returned nil; now it returns 713 characters.

Migration notes worth keeping:

- `defdyn` must declare at **expansion** time as well as run time — the compile
  pass resolves namespaces *after* macroexpansion, so the ambient mark has to
  exist before the `def` head it emits is qualified. And `scan_def_names` has to
  drop names the file declares `defdyn` even when the same file also `def`s them
  (`std/tool/test.blsp` reads `*test-filter*` at the top, declares it in the
  middle, `def`s it near the bottom) — otherwise the pre-scan qualifies the early
  reads to `test/*test-filter*`.
- Making the prelude's *own* registries dynamic was tried and **reverted**:
  `*load-path*` as a `defdyn` changed how `:isolated` tests see it and brought the
  project_test load-path race back. They stay plain root globals with root setters
  (`set-load-path!`, `add-load-path!`, `record-module-doc!`) — only root code can
  rebind a root name now, which is the honest consequence of ADR-151.
- Cross-module knobs `def`'d *inside a function body* (`*project-description*`,
  `*project-repository*`, set only in `project-setup`) were missed by a top-level
  scan and surfaced as a `nest publish` failure. The error message
  ("`*project-description*` is defined as `project/*project-description*` — add
  `(:use project)` …") pointed straight at it, which is the hint machinery paying
  for itself.

Green: 3266 in-language tests, `nest check` clean, 866 Rust tests (the two
`cli::distribution` "flakes" in the first run were two more `~w` pins my grep
missed — the pattern compiler's new error named them exactly).

Left open deliberately: `sig`/`defrecord` adoption (zero `(sig …)` declarations
across 46k lines of `std/`, `defrecord` used 5 times, all in the prelude and the
docs tool) — either commit and annotate `std/`, or replace the out-of-band
declaration with an inline annotation in the param list. That is a design call,
not a cleanup, so it stays for its own session. Also left: the `car`/`cdr`,
`lambda`, `concat`, `reductions`, `intersperse`, `all-globals` aliases and `cond`'s
`else`/`:else` dual spelling — one spelling each is the smaller language, but
these are taste, not safety, and breaking them buys nothing a hint doesn't.

## 2026-07-25 — conformance corpora, suite 4: UCD, and two new Unicode primitives

Wired the Unicode Character Database conformance files (Unicode 16.0.0, pinned to the
version `unicode-segmentation` / `unicode-normalization` were generated from — testing
against a newer UCD than the crates implement produces failures that are skew, not
bugs, so `data/VERSION` and the runner's first test pin it).

**Needed new language surface, agreed with the user first.** Brood exposed neither
grapheme segmentation nor normalisation — only `display-width`, which segments
internally but doesn't expose the clusters. Added:

- `(string->graphemes s)` → vector of extended grapheme clusters. The sibling of
  `string->codepoints`, and the unit a human means by "a character": `"e"` + U+0301 is
  two code points and one cluster; a flag emoji is four and one. This is what editor
  cursor motion must step by — stepping by code point splits a cluster and corrupts
  the buffer, which is the bug every editor writes once.
- `(string-normalize s form)` with `form` ∈ `:nfc :nfd :nfkc :nfkd`. One primitive
  with a form keyword rather than four functions (ADR-011). Brood's `=` is
  byte-structural, so `"é"` as U+00E9 and as U+0065 U+0301 compare unequal until
  normalised. New dep: `unicode-normalization` (tiny tables, same tier as the two
  Unicode crates already linked).

**NormalizationTest: ~19,000 cases, all pass** — and the runner checks UAX #15's full
conformance *closure*, not just `NFC(source)`: every one of the five columns normalised
into every form must yield that form's column. That's where idempotence gets tested,
and idempotence is where normalisers actually break.

**GraphemeBreakTest: 602 cases, one failure, and it's upstream.**
`÷ 2701 × 200D × 2701 ÷` — UAX #29 rule GB11 joins a ZWJ sequence when both sides are
Extended_Pictographic, and Unicode 16 gives U+2701 UPPER BLADE SCISSORS that property,
so it is one cluster; `unicode-segmentation` 1.13.3 (current release) omits U+2701 from
its table and returns two. The rule is correct for every other pictographic tested
(U+270A, U+2764, U+1F468, U+1F3F3 all join), so it's a table gap around the U+2700
dingbats — worth an upstream report, not a workaround. Excluded from the sweep and
pinned by a test asserting the *current* behaviour, so the exclusion fails loudly the
day the crate fixes it.

Method note worth keeping: my first hand-written property test asserted
`(string->graphemes "e\u{301}f")` equals `["é" "f"]` — with a literal `é` in the source,
which is precomposed U+00E9 and therefore a different string. The test failed with
"actual" and "expect" printing identically. Spell both sides with escapes in any test
that touches normalisation; the corpus is the only honest source of truth here.

Suite green: 3294 in-language tests, `nest check` clean.

## 2026-07-25 — conformance corpora, suite 5: csv-spectrum, and three suites ruled out

**csv-spectrum** (BSD-2-Clause, 12 documents) — the first corpus pointed at a
**pure-Brood** subject rather than a Rust crate behind a thin wrapper, and it found a
bug on the first run.

**A CRLF inside a quoted field was being rewritten to LF.** RFC 4180 §2.6: a field
enclosed in quotes may contain CRLF, and it is *content*. `csv--parse` swallowed the
`\r` in its `:quoted` state along with the ones that genuinely are line endings, so
any CSV with a multi-line quoted cell — the corpus's `newlines_crlf`, and anything
exported from Excel on Windows — silently lost its carriage returns and did not
round-trip through `csv-parse` → `csv-emit`. Line-ending normalisation now happens
only in the `:unquoted` and `:quote-seen` states. Regression case added to the runner,
including the emit→parse round trip.

`location_coordinates` is excluded: broken upstream two ways — its expectation is a
bare JSON object where every other one is an array of row objects, and its phone
number doesn't match its own CSV.

**Three suites ruled out, recorded as blocked rather than quietly dropped.** Each
targets a specification Brood isn't implementing, so wiring them would produce a wall
of skips that reads like coverage:

- **regex** (Fowler / rust-lang testdata) — `std/regex` is a deliberate subset: no
  ranges `[a-z]`, no captures, no `{m,n}`, no backreferences. ~95% skips.
- **CommonMark** — there is no `std/markdown` at all.
- **WHATWG URL** (WPT `urltestdata.json`) — `std/url` is RFC 3986 parsing with no
  base-URL resolution, IDNA, or per-component percent-encode sets. Different spec.

Method note that generalises: the scoring so far tracks *what the code is*, not what
the corpus is. The two suites over Rust crates behind thin wrappers (parse-number →
`f64::from_str`; UCD normalisation → `unicode-normalization`) found nothing in Brood
and one upstream gap. The three over Brood-side logic (dectest's scale handling,
`std/json`'s string states, `std/csv`'s quote states) found a bug each. Prioritise the
remaining pure-Brood targets accordingly.

Suite green: 3328 in-language tests, `nest check` clean.

## 2026-07-26 — KI-11 fixed, and three more corpora (one found a `pow` bug)

**KI-11 fixed — and it was not a missing guard, it was a guard that stopped applying.**
`JIT_NATIVE_DEPTH_LIMIT` (1500) and `Heap::jit_native_depth` already existed and *were*
checked by both dispatch entry points. But `jit_run_fast_link` restored the counter to
the caller's level as soon as the native callee returned and *then* handled the outcome —
and three outcome arms re-enter the evaluator while that frame is still on the native
stack: the outcome-4 tail-chain follow-through (`apply_value`) and the deopt/preempt
re-runs (`vm_resume_deopt` / `vm_apply`). So a chain of tail-calling delegators oscillated
between `depth` and `depth+1` forever while the native stack grew. The cap never tripped
because the depth never climbed. Fix: `jit_native_reenter` re-raises the depth for the
duration of each re-entrant call. Also renamed the local to `native_depth`, because the
deopt arm binds a `depth` of its own (the checkpoint's VM stack depth) that shadowed it —
exactly the confusion that hid this.

Why no existing depth test caught it: the trigger needs a **cycle** where at least one
link is a plain tail-call delegator (so its native returns outcome 4) *and* the cycle is
entered from a non-tail position. Plain deep self-recursion — 20-local frame, mutual,
building a 20k-deep nested value — all stay under the cap and run to 200,000 fine. The
minimal repro is six lines; `std/json`'s
`json--value` → `json--array` → `json--array--acc` → `json--value` is the shape in the
wild, with `json--array` as the delegator. A/B against a pre-fix binary: aborts at 20,000
pre-fix, runs to 50,000 post-fix. Regression test in
`tests/jit_tail_chain_depth_test.blsp`; the hot outcome-0 path is untouched so there is no
perf exposure.

**A follow-on the fix exposed.** With the abort gone, the two 100k-deep JSONTestSuite
documents now do real work — ~400 ms standalone, but >120 s inside the full parallel
suite, a >250× slowdown steeper than contention explains and suggestive of a GC cost
superlinear in graph *depth*. Putting them in an `:isolated` test made it worse (isolated
units run on the runner process, so taking the ceiling there killed the whole run). They
are skipped in the sweep with the property covered by a synthetic 5,000-level case, and
the slowdown is logged in ROADMAP as its own question. Worth a `BROOD_GC_TRACE` session:
if it is real it affects any workload holding a deep structure.

**Three more corpora wired.**

- **Kuhn's UTF-8 stress test** (CC BY 4.0). No findings — Brood delegates to
  `String::from_utf8` and `slurp` correctly *raises* on a malformed file rather than
  substituting U+FFFD. The value is the gate plus an explicit record of which classes are
  accept-vs-reject, since that is the part a future hand-rolled decoder or a `slurp`
  "convenience" would get backwards: overlongs rejected (an overlong `/` = `C0 AF` is a
  path-traversal vector), noncharacters *accepted* (U+FFFF is a legal code point).
- **NIST CAVP** (public domain): SHA-1/256/384/512 ShortMsg + LongMsg + ~1,250 HMAC
  cases. No findings; the digests come from CAVP-validated crates, so the exposure was
  the wiring, not the compression function. Two format traps worth knowing: `Len = 0` is
  spelled with a placeholder `Msg = 00` (the message is *empty*, not the byte `0x00`), and
  `Tlen` **truncates** the MAC. Wycheproof deliberately not wired — its value is
  ECDSA/AES-GCM/RSA, none of which Brood implements.
- **Paranoia** (Kahan), ported to Brood — and it found a bug.

**`pow` lost the entire subnormal range.** A negative exponent computed
`(/ 1 (pow base (- exp)))`, so when the *positive* power overflowed to `inf`, `1/inf` was
0.0: `(pow 2.0 -1074)` returned `0.0` where 2⁻¹⁰⁷⁴ is perfectly representable (`5e-324`),
and every exponent past −1023 was wrong the same way. An int base failed for the sibling
reason — `base^|exp|` becomes a bignum rather than `inf`, and *its* reciprocal underflows —
so the test has to be on the reciprocal, not on the power. Fixed in the prelude (Brood, not
Rust) by splitting the exponent in half, which keeps every intermediate in range and is
exact for a radix-2 base.

Paranoia also pinned Brood's one deliberate IEEE 754 departure: **division by zero raises**
(E0040, with a hint) rather than yielding infinity. Overflow still produces infinity, so
infinities exist — they just are not reachable by dividing, which is why the port
constructs `inf`/NaN with `bits->float` instead. That is now asserted rather than assumed.

Method note: the Paranoia port's own first draft failed for four *different* reasons that
were all mine, not Brood's — a mis-transcribed radix derivation, a precision loop testing
the wrong direction, `(/ 1.0 0.0)` used as an infinity constructor, and subnormals built
with the very `pow` that turned out to be broken. A derivation-based test is worth more
than a table precisely because it fails loudly when your model of the machine is wrong.

Suite green: 3385 in-language tests, `nest check` clean.

### Follow-ups from the same day (2026-07-26)

**KI-11 needed a second half.** With the counter fixed, release was clean but the *debug*
build still aborted: `JIT_NATIVE_DEPTH_LIMIT` is a frame **count**, and a count is only
right for one frame size — 1500 levels is a few MB of the 16 MB worker stack in release and
several times that in debug. The same root flaw as the bug, one level up. Added
`jit_native_headroom_ok`, which probes `stacker::remaining_stack()` and refuses a new native
link below a 512 KB margin, with the count cap kept as the cheap first test and the probe
skipped below 64 levels so the hot shallow path pays nothing. Measured flat (primes-to-200k
57→58 ms, fib-30 5→6 ms). This also covers a case the original cap never anticipated: a host
embedding Brood on a smaller thread stack.

**Suite scheduling, twice.** The 100k-deep JSON documents are ~400 ms standalone but >120 s
inside the parallel suite; an `:isolated` test made it *worse*, since isolated units run on
the runner process and taking the ceiling there kills the whole run. They are skipped with
the property covered by a synthetic 5,000-level case, and the >250× gap is logged in ROADMAP
as a possible GC-depth pathology. Separately, the UCD normalisation sweep (19,000 cases × 6
normalisations) was one test at 44 s in debug — split along the corpus's own `@PartN`
sections, which keeps every case, parallelises them, and drops the slowest test to 2.1 s;
the two corpus files are now read once at load rather than once per part (peak 67→32 MB).

**`.config/nextest.toml`: `binary(suite)` 300s → 600s.** The corpora add ~67 s of debug-build
work to the single wrapper case. Noted in the config that the next person should check for a
slow *case* before bumping the budget again.

## 2026-07-26 — `sig` adoption (the pilot that broke four things), and the alias trims

Finishing the surface review's last open item. Two decisions taken by the author:
adopt `(sig …)` in `std/` rather than redesign or delete it, and trim the redundant
aliases — keeping `car`/`cdr` and `lambda`.

**The `sig` pilot did its job by failing.** 23 signatures across `std/path` (14),
`std/set` (7), and `std/json` (2), and the *attempt* surfaced four defects that
zero adoption had hidden (ADR-153): `bytes`/`decimal` were unspellable as types
(so no bytes module could ever be annotated); `sig!` couldn't expand early in the
prelude because it called `index-of`, which the prelude defines at line ~2770;
`BROOD_CONTRACTS=1` turns a declaration into a rebinding, so a `sig` above its
`defn` killed the module load; and that guard's `bound?` check asked about the
unqualified name, reporting "not defined yet" for a correctly-placed sig. The
fourth finding is the one to keep in mind: **a prelude function cannot carry a
`sig`** — the contract wrapper captures a local frame and the prelude freeze
forbids that for a shared closure. The 14 prelude annotations came back out.

Payoff, verified: a wrong call in another module warns (`path/basename: argument 1
expects string, got 42`), the result type flows (`(string-length (is-dir? "/tmp"))`
→ "expects string"), correct calls stay silent, and `BROOD_CONTRACTS=1` now throws
at runtime for the same mistake.

**Trims.** `concat`/`intersperse`/`reductions`/`all-globals` removed (~26 call
sites migrated; `concat` and `all-globals` turned out to be one-line pass-through
wrappers, not `def` aliases). `cond` stopped special-casing `:else` — 96 sites
migrated to bare `else`. Worth writing down *why* that isn't a removal of
behaviour: `:else` still catches, because a keyword is self-evaluating and truthy,
exactly as `(cond … 42 x)` catches. Only bare `else` ever needed a case in the
macro. Making `:else` an *error* would have meant **adding** machinery that also
rejects `true`, so it wasn't done.

Two mechanical notes for next time: a regex sweep over `(name ` call syntax misses
**value-position references** — `(apply concat …)` in `defmodule--use-forms` took
the prelude down on the first run — and it happily rewrites your own prose, which
mangled the comment explaining the `:else` decision.

Green: `nest check` clean; 3390+ in-language tests pass. The UCD normalisation
conformance cases (untracked, being split into chunks upstream of this work) time
out against the 120 s per-test watchdog in a debug build — that file is the
author's in-flight work, not this change; it references none of the trimmed names.

**Left red, deliberately, rather than guessed at.** `cargo nextest run` fails on
`brood_suite_passes`: the UCD normalisation sweep times out on the framework's 120 s
per-test ceiling inside that debug-build wrapper under full-workspace load. It is 1.1 s
standalone in release and 31 s for the whole file in debug — and sampling it from ~16,000
cases to ~1,000 did not move the wrapper result, which is what rules out test size. Same
signature as the 100k-deep JSON documents (>250×). Four shrink-the-test cycles bought
nothing, so I stopped: it is a contention/GC question, logged in ROADMAP with the evidence
and the next diagnostic step, not a sizing question. `nest test` (release, 3400 tests) is
green, as are `nest check` and `cargo fmt --all --check`.

## 2026-07-26 — KI-12: the prelude froze a RUNTIME handle as PRELUDE (and `:conformance` now buys budget)

Two failures were left after the syntax work; both are closed.

**KI-12 — a wrong string in every build.** `(println (pr-str *load-path*))` returned
`("A list of the given arguments.")` — `list`'s docstring — instead of `(".")`. The
list was right; its **car** was a different object, and *which* object moved with
heap layout (`("ret")` under `BROOD_GC_VERIFY=1`, the symbol `xs` if the prelude
wrote `'(".")`). Correct under `BROOD_VM=0`. A binary predating the day's work failed
the same way with `cond`'s docstring, so it was latent, not a regression.

Reading the freeze twice found nothing, so I instrumented it, which answered it in
one line:

```
[freeze] *load-path*: Pair region=0 idx=55990 car=Str region=2 idx=60
```

A LOCAL pair holding a **RUNTIME** string. `to_prelude` re-tags a handle by keeping
its slab index and changing its region — sound only for LOCAL handles, since the
builder's slabs *become* the prelude region — and it was applied unconditionally. The
VM interns constant-pool literals into RUNTIME so compiled code is shareable, so a
prelude global built by compiled code held one, and the re-tag pointed it at an
unrelated prelude string. The tree-walker passes the LOCAL read-form string, hence
its correctness — the one clue that mattered.

Fix: `localize_for_freeze` deep-copies any non-LOCAL part of a global's reachable
graph into the builder's slabs *before* the sweep, and `to_prelude` now re-tags LOCAL
only (unreachable boot garbage legitimately holds RUNTIME handles; flipping those was
the damage). A `debug_assert` on the root bindings turns a future gap into a freeze-time
failure. Costs one walk per *source* boot — freeze 5.0 → 8.7 ms, source-boot peak 3.7 →
10.6 MB, cached boot unchanged.

What it had been costing: filesystem module lookup from the default load path never
worked. Nothing noticed because `require` finds std modules via `%builtin-module` and
a project run replaces the path. `brood-lsp`'s
`completes_module_names_in_require_and_use` was the only reader, and is green again.

Two smaller things fell out of the same investigation: `to_prelude` had no `Bytes`
arm (a `#b"…"` literal in a prelude global would have kept a LOCAL tag — latent, no
prelude form produces one today), and `eval_forms`' per-form arena reset (ADR-016)
was dead code resting on a false premise ("globals live in PRELUDE/RUNTIME"), so it
is gone with the reasoning recorded where it was.

**`:conformance` now buys batch budget.** `*test-timeout-ms*` is a *batch* wall
deadline, and the external corpora exceed 120 s in a debug build — so `nest test`
hard-killed UCD normalisation, then JSONTestSuite, as legitimately-long work. `:slow`
was already in the tags but the runner ignored it entirely. Now a unit tagged `:slow`
**or** `:conformance` raises its batch to `*test-slow-timeout-ms*` (900 s);
`:conformance` counts because a corpus sweep is thousands of cases by construction and
tagging each new one `:slow` as well is a step the next author will forget. Untagged
batches keep the 120 s ceiling, which is what catches a hang.

Also fixed here: `introspect::loadable_modules` ran a Brood snippet using `concat`,
removed in the alias trims — and its error was swallowed by `if let Ok(v)`, so module
completion silently returned nothing. The alias sweep had covered `.blsp` files and
Rust *test* snippets but not Brood embedded in Rust *library* code; the checker's
`(concat …)` element-type rule and a now-duplicate catalog row went too.

## 2026-07-26 — the syntax finalization, from downstream: three brood defects it exposed

Pulled `d471359` (the syntax finalization) and swept the 13 sibling projects. Eight were
broken; the two mechanical migrations fixed five, and the other three were **brood bugs
the projects found**, not project bugs.

### The mechanical part

`~x` → `^x` (ADR-150) and `concat` → `append`. The pin rewrite needs quasiquote context,
so it was a scanner, and ADR-150's "167 pins migrated" made a free oracle: run the
scanner over `std/` + `tests/` at the *previous* commit and diff its output against what
landed. It reproduced **167** — and 207 of 209 files matched pin-for-pin, with the six
differences all *new* text in that commit (the pin docs, scaffold templates). Zero cases
where it rewrote a `~` the real migration had left alone, which is the direction that
breaks quasiquote. Worth the ten minutes before touching six repos.

### Defect 1 — `&optional` defaults were never namespace-qualified

`(defn project-root (dir &optional (limit *project-search-depth*)) …)` raised
`unbound symbol` — 25 failures in brood-edit alone. Param lists were passed through the
resolver verbatim, which is right for the *binders* (they are not references) and wrong
for a default *expression*, which is ordinary code in the defining module. It resolved at
call time in whatever namespace the caller happened to be in.

Reduced to: a default reading a plain `def` fails, an earmuffed `def` fails, a `defdyn`
works, the same read in the body works. So this predates the finalization — it dates to
ADR-065 (2026-05-30, `out.push(params); // verbatim`) and earmuffs *masked* it, since an
ambient `*knob*` resolved from anywhere. ADR-151 removed the mask and the two-month-old
hole surfaced. `resolve_param_defaults` resolves defaults while accumulating earlier
binders as locals, so `(defn rect (w &optional (h w)) …)` — a documented shape — keeps
working. Seven cases in `tests/namespace_test.blsp`.

### Defect 2 — the tightened `defmodule` header rejected `(:implements …)`

`defmodule--clause-heads` listed `:use`, `:use-internals`, `:alias`. But `:implements` is
a **checker** annotation that brood's own `types/check/protocol.rs` reads straight out of
the header; the loader's only job is to tolerate it. Making unrecognised clauses a hard
error therefore turned every protocol-implementing module into a load error — willem's
whole suite. The clause is now accepted and named in the error text.

### Not a third defect — a misread, fixed properly upstream

`brood_suite_passes` was also red on the UCD "Part1" conformance test, and I read the
120 s ceiling as a *per-test* limit: one test measured 225 s, so I made the walk cheap
(the `nt--test-line?` scan over 20,000 lines costs >120 s in a debug build on its own —
provable by running a slice with the modulus set so high that no case is tested at all,
which is still killed). It passed, but the diagnosis was wrong: `*test-timeout-ms*` is a
deadline for a whole *batch* of parallel workers, not for one test. A single test is its
own batch, which is why my measurement fit both readings and never discriminated between
them. The real fix — `*test-slow-timeout-ms*`, a raised budget for a `:slow`/`:conformance`
batch — landed upstream in 5a76f90 while I was working, so I dropped my change and took
theirs, and Part1 runs in full again rather than sampled.

### Left alone

Thirty-two `.blsp` files arrived unformatted; formatting `conformance_ucd_test.blsp`
reflowed 97 lines around my 45, so I reverted that and kept the edit alone — the
unformatted files are the author's to sweep.

## 2026-07-26 — ergonomics & conciseness pass (ADR-154): add the sugar, cut the surface

A whole-language review for conciseness/ergonomics. The core came out clean — the
friction was all in the library/macro surface, so every change here is a pure prelude
macro or a rename, no evaluator change. Two gaps dominated the evidence: **492 `(str …)`
+ 151 `(error "…" x)` sites** (no interpolation), and **83 top-level `--acc`/`--loop`
helpers** (~4% of `std/`) that exist only to be a hand-written tail loop.

**Added (all pure macros, zero core cost):**
- **`fmt`** — `(fmt "x={x} sum={(+ a b)}")`, parsed at expand time into a plain
  `(str …)`. `{{`/`}}` literal braces; braces nest in a hole. Not a reader sigil
  (`#"…"`/`#b`/`#{` are taken and a macro is the ADR-006 way).
- **`if-let`/`when-let`** (test the source value via a temp, so destructuring targets
  work), **`some->`/`some->>`/`cond->`/`cond->>`/`doto`**, **`run!`**.

**Local loops — no `loop`/`recur` (prototyped, then dropped).** The 83 helpers
motivated a `loop`/`recur` macro; built it, reshaped to a Scheme named-let, then
**removed both.** Reasoning: (1) a `loop` macro is a Lisp-1 reserved word (Lisp-2
would free it but taxes every higher-order call with `funcall`/`#'` — rejected);
(2) `recur` exists in Clojure mainly to work around the JVM's lack of TCO — **Brood
has proper tail calls**, so `(defn f (x) (f (dec x)))` is already O(1) and recur's
reason evaporates; (3) with recur gone, `loop` is just `letrec` sugar, not worth a
reserved word. So a local loop is a `letrec`-bound closure called by name
(`(letrec (go (fn (i acc) … (go …))) (go 0 0))`), closing over the enclosing scope;
`defn` covers loops needing no locals. The `loop`/`recur` hint points to `letrec`.
(The general reserved-word cost of other macros + the shadowable-operators idea are
in deferred.md #7; ADR-154.)

**Cut (one spelling each, no users so free):** `string-contains?`→`includes?` (306
sites; superset merge), `string-index-of`→`index-of &optional from`,
`string-last-index-of`→`last-index-of`, `string-capitalize`→`capitalize`,
`string-upcase`/`downcase`→`upper`/`lower`, `flat-map`→`mapcat`, `length`→`count`,
`entries`→`map-pairs`, `read-file`/`write-file`/`append-file`→`slurp`/`spit`/
`spit-append`, `path-exists?`→`file-exists?`, `working-dir`→`cwd`, `host`→`hostname`,
`some?`→`any?` (frees the Clojure-surprising name), and the deprecated `:refer` marker
(→ `:only` only). Reverses ADR-153's deliberate keep of `car`/`cdr`.

**Kept on purpose:** `multimap-`'s prefix (dropping it breaks *internal* resolution —
a bare `get` inside namespace `multimap` would resolve to `multimap/get`, not the
prelude) and `set/conj` (shadows only under `(:use set)`, the Clojure `clojure.set`
contract, not a defter).

### The one trap the mechanical rename hit

Renaming call sites of common-word names (`entries`, `host`, `length`, `car`, `cdr`)
with a *head-position* regex (`(?<=\()NAME`) also matched **let-binding and single-param
positions** — `(let (entries …))` and `(defn f (entries) …)` both put the name right
after `(`. So three single-param functions (`package--lockfile-content`,
`package--index-by-name`, `coverage--line-index`) had their `entries` param silently
renamed to `map-pairs` while the body kept using `entries` → runtime `unbound symbol`.
`nest check` *missed* two of them (the `map-pairs` param shadows the global, confusing
the checker), so the in-language suite — not the checker — is what caught them. Lesson:
a symbol rename by regex is unsafe for names that double as locals; the real fix is an
AST-aware rename, and the suite is the backstop. All fixed; `nest check` clean, suites
green. Full coverage in `tests/ergonomics_test.blsp`.


## 2026-07-26 — conformance corpora, suite 13: the Gabriel benchmarks, and a hole in the engine gate

Every corpus so far feeds hostile **data** to one parser. This one runs whole
**programs** with known answers, which is the only way to get an oracle for the
evaluator itself. Eight of the Gabriel/Larceny benchmarks are ported to Brood
(`tests/support/gabriel/*.blsp`) and checked against upstream's own expected outputs:
`nboyer`, `chudnovsky`, `mazefun`, `deriv`, `takl`, `cpstak`, `nqueens`, `primes`.

**No wrong answers — on either engine.** `nboyer` reproduces upstream's published
rewrite counts exactly (95,024 / 591,777 / 1,813,975) and `chudnovsky` its ten exact
50-to-500-digit integers. That matters more than a boolean: upstream reports rewrites
deliberately, "because it is too easy for a buggy version to return the correct boolean
result," so matching a six-figure count pins rule *order*, the unifier and the tautology
walk together.

**It found a checker bug — KI-13, and the first defect these corpora have turned up
*outside* the code under test.** `nest check` **hangs** on the `deriv` port: not a loop, an
exponential. Cross-module return-type inference for an undeclared recursive callee grows
with the number of `cond` branches that build nested list structure — 2/3/4/5 branches cost
105 ms / 105 ms / **8.7 s** / did not finish in 900 s. The same call *inside* the defining
module is instant, so it is specifically `sig_of` → `infer_sig` → `expr_ty` across a module
boundary, where nothing bounds the *size* of the inferred `Ty` (`InferGuard` breaks
recursion *cycles* correctly — a different thing, and `expr_ty` already has a depth cap).
That matters more than a warning would: `nest check` is a CI gate and the same code backs
the LSP, so an editor hovering the call site hangs too, and the trigger is ordinary code.

Worked around rather than fixed, deliberately — a declared signature is consulted *before*
body inference (`declared_heap_sig`), so `(sig deriv (any -> any))` takes the case from
>900 s to 105 ms. That is what the port carries, with a comment saying the sig is
load-bearing so nobody deletes it as decoration; declaring a public API's type is right
anyway (ADR-153). The real fix is to cap inferred-type size and widen past the cap —
widening an over-approximation is sound by construction, so it cannot introduce a false
positive, only lose precision on the pathological shape. `MAX_INFER_DEPTH` is the
precedent. Repro + table in `docs/known-issues.md`.

**The second find was in the harness, not the language.** `BROOD_VM=0` does **not**
give the in-language suite tree-walker coverage. A test body run by `nest test` (or
`brood --test`) with it set shows no slowdown at all, and `BROOD_JIT_DUMP_IR=1` lists its
arms reaching the JIT; the same function at top level via `brood file.blsp` interprets
correctly, emits zero JIT arms and runs ~10× slower. The env var gates how a *top-level
form* is run, and the framework invokes each test as an already-compiled closure. So
`make test-both`'s tree-walker leg does not exercise the ~3400 in-language cases the way
its comment implies — real per-expression agreement comes from `differential.rs`, which
pins the engine with `set_forced_engine` rather than the env var.

Hence two runners for this corpus: `tests/conformance_gabriel_test.blsp` (upstream's
oracles, on whatever engine the suite uses — always the VM) and
`crates/lisp/tests/gabriel_engines.rs` (the same oracles under `set_forced_engine`, so
both engines really run). Whether the framework should honour a forced engine for test
bodies is now a decision on the ROADMAP; it would widen the gate a lot, and cost a lot
(debug tree-walker: `nboyer` n=0 is 38 s against 0.25 s on the VM).

**Two engine limits measured on the way.** The debug tree-walker spends ~12.6 kB of
native stack per frame, so `primes<=1000` — 999 levels of non-tail `interval-list` —
trips the 12 MB budget there with a clean `recursion too deep`. Correct behaviour, and
release handles the same call fine, but it is why the Rust runner sieves to 100 (the
first 25 entries of the *same* vendored list, so the oracle is unchanged). And upstream's
current input sizes are calibrated to *time* a native-compiling Scheme, not to be
checked: Chez needs 3.97 s for one iteration of `takl:40:20:12`. Where that was true the
runner uses an older stanza upstream still keeps in the same `.input` file, or a
published table (`nboyer`'s header, OEIS A000170 for `nqueens`) — recorded per test,
because the provenance of each expected value is the whole basis of the suite.

**What was not ported, with reasons, rather than left as a todo.** `gcbench` is descoped
twice over: its correctness predicate is literally `(lambda (result) #t)`, and its
subject — `Populate` building a tree top-down "assigning to older objects" — is the
write-barrier case Brood cannot have by construction, since immutability is what
guarantees old never points to young. `destruc` is the "destructive operation benchmark"
where the aliasing *is* the program. `peval` (twelve `set-car!` sites rewriting an AST in
place) and `earley` (25 `vector-set!` sites) are real rewrites with a single-output
oracle that would not localise a mistake. `nucleic` is the best next candidate: mostly
functional already, and a 3,485-line float oracle.

Also here: `corpus-forms` joins `corpus-lines` in `tests/support/corpus.blsp`, for the
corpora whose data *is* Lisp — it reads the `.input` files so expectations are upstream's
bytes rather than retyped.

### Addendum, same day — what the corpus run turned up about `nest test` itself

Wiring the corpus meant running the whole suite a lot, which surfaced two things that have
nothing to do with the Gabriel programs and are recorded here so the next person does not
re-derive them.

**`target/release` was stale, and it lies quietly.** The binaries in `target/release` dated
from 09:07, predating that day's three commits, so the first suite runs reported five
failures (`sig_adoption` ×2, `private_test`, `macro_harden`, JSON `n_`) that were simply
features present in the source and absent from the binary. This is the `-p brood` trap from
`CLAUDE.md` wearing a different hat: there, an A/B compares stale binaries; here, a *test
run* does. Rebuilt with `cargo build --release --bin brood --bin nest`, four of the five
went away. Worth checking binary mtime against `git log -1` before believing a suite result.

**The deep-JSON slowdown is real, is not confined to the debug wrapper, and the
"misdiagnosis" verdict on it was itself wrong.** The two 100k-deep JSONTestSuite documents
were un-skipped on the reasoning that `*test-timeout-ms*` is a *batch* deadline which blamed
whichever worker reported last — so "nothing here is slow". Measured on a current-source
build: `nest test` (the **release** path) runs ~15 minutes and ends with `every n_ document
is rejected` **hard-killed at 900 s**, one thread pinned at ~90% CPU, while the same file
standalone passes in seconds. Reproduced with the corpus file removed from `tests/`
entirely, so it is not the new work. The batch-deadline story explains a *mislabelled*
failure; it does not explain 900 s of real CPU. Two changes compounded: the documents came
back into the sweep, and `:conformance` now raises the batch to the 900 s
`*test-slow-timeout-ms*`, so what previously failed fast at the 120 s cap now grinds for
900 s — which is why the old binary finished the same tree in 318 s and the new one does
not finish at all.

So `nest test` on main is currently red *and* slow, for the reason already open in ROADMAP
(a superlinear cost in graph depth, suspected GC). Left exactly as found: the roadmap entry
says not to "fix" this by shrinking the tests again, and re-skipping the documents would
hide the one case that reproduces it. `ptrace_scope` blocks `gdb -p` without root, so a
stack sample needs `sudo sysctl -w kernel.yama.ptrace_scope=0` or running the case under
gdb from the start.

## 2026-07-26 — the ADR-154 rename, from downstream: what a mechanical rename gets wrong

Swept the 12 sibling projects against the trimmed surface. Nine needed migration
(brood-edit 163 failures, hatch 216, brood-chat 28, brood-terminal 16, willem 7,
hatch-demo dead at load) and all are green again. The rename map itself was ADR-154's
own list, applied by script. The interesting part is the two ways the script was wrong,
because both produce a **passing** test suite while being wrong.

### `entries` is also a variable name

`entries`→`map-pairs` hit 66 sites in brood-edit, 29 in hatch, 18 in brood-chat. Many
were not the prelude function at all: they were locals, `defn` parameters and prose
(`presence--without (entries pid)`, `(filter git--staged? entries)`). A whole-symbol
replace renamed the binder AND its uses, so the code stayed internally consistent and
**every test still passed** — with user variables now shadowing a primitive. Nothing in
the suite could have caught it; only reading the diff did.

Reverting it took three passes, each revealing the next: bare references first, then
`(let (map-pairs …` binding lists, then `(defn f (map-pairs pid)` parameter lists. A
parameter list and a call are the same shape in a Lisp, so "only rewrite in call
position" — the obvious guard — cannot tell them apart. What finally worked: a genuine
call is `(map-pairs m)` *and* the name is not introduced as a binder anywhere in the
form. For a rename of an ambiguous name, prefer a scope-aware pass, or read every hunk.

### `(let (host …))` looks exactly like a call

The same call-position heuristic renamed the binder in
`(let (host (get opts :host "127.0.0.1")) … (tcp-connect host port))` and not the use,
because a binding list opens with `(` directly before its first binder. store-postgres
lost 22 tests to it. Three more hits were inside comments and a docstring, where the old
name is prose and correct.

### The rest

- A genuinely stale test: brood-edit asserted completion offers `car`, removed by
  ADR-154. Retargeted at `capitalize` — the behaviour under test is prefix completion of
  globals, which any surviving `ca…` name exercises.
- `hatch-demo` and `willem` consume hatch and store-postgres as **git** deps, so their
  `_deps` copies stay broken until the dependency is pushed and `nest update` run. Order
  matters: store → store-postgres → hatch → hatch-demo/willem.
- Reference docs swept for names that no longer exist: `lambda` as an `fn` synonym,
  `all-globals` as an in-language function (it survives as an *MCP tool* over
  `global-names`, which is why the mcp.md row stays), and the `concat` alias in both
  `brood-for-claude.md` and the writing-brood skill. Left `devlog.md`/`decisions.md`
  alone — there the old names are the record of what was true, and editing them would
  falsify it.

## 2026-07-26 — the message rows: `receive` clause bodies move into the calling function (ADR-155)

Picked up the four open Elixir-parity performance rows (`nqueens`, `ring`/`pingpong`,
`bintree`, `loop`). Baseline on a fresh `--bin brood` build, `make`-installed:
`loop` 50 ms · `bintree` 119 ms · `nqueens` 94 ms · `pingpong` 249 ms · `ring` 1376 ms.

### Where the message time actually was

The useful move was to **take the scheduler out of the picture** — a single process
sending to itself and receiving, 200k times, which exercises the whole message path
with zero cross-process handoff. Bisected against progressively smaller programs:

| step | 200k iters | delta |
|---|---|---|
| bare tail loop | 14 ms | — |
| + build `[:ping k]` | 25 ms | 55 ns/iter |
| + `send` to self | 87 ms | **310 ns/iter** |
| + `receive` | 251 ms | **820 ns/iter** |

At 1.2 µs per receive this matched `pingpong`'s per-receive cost almost exactly — so
**`pingpong` was not paying for scheduling at all**; wake elision and ADR-135 had
already flattened that. It was paying for `receive` itself. Two knobs then said
something surprising: `BROOD_NO_JIT=1` (255 ms) and `BROOD_NO_HOF_JIT=1` (259 ms)
were **indistinguishable from the default** — the hot message path was running with
no native code whatsoever — while `BROOD_NO_HOF=1` was 804 ms, so the cached-arm HOF
path was the only thing keeping it afloat.

The cause was one design decision doing double damage. `receive` wrapped every clause
body in a `(fn () body…)` thunk so the body could run after `%receive` committed to
the message. Measured directly: building + calling such a thunk costs **235 ns**
against **50 ns** for a small-vector protocol. And because `Inst::MakeClosure` is not
in `chunk_in_jit_subset`, building it made the **entire matcher arm** ineligible to
lower — which is why the JIT knobs did nothing.

### The fix

Split selection from execution (ADR-155): `%receive` drops to arity 2 and only
answers `[idx var…]` — which clause matched, and what it bound — with `nil` for no
match and `nil` on timeout. The macro emits every **body at the call site** and
dispatches on `idx`. Bodies now compile into the owning arm, so a receive loop's
self-call is an ordinary tail call there; the matcher allocates a vector
(`Inst::MakeVector`, in the subset) instead of a closure, so matcher arms lower.

The `tail_call` counter for `pingpong` tells the story: **400,309 → 473**. Both
matcher arms show up in `BROOD_JIT_DUMP_IR` as tiering `<closure>` arms now.

| | before | after | |
|---|---|---|---|
| `ring` N=200 | 1376 ms | **720 ms** | **−48%** |
| `pingpong` N=100k | 249 ms | **194 ms** | **−22%** |
| isolated receive | 820 ns | **615 ns** | −25% |

`loop`/`bintree`/`nqueens`/`spawn`/`fib`/`sieve` all unchanged (controls). Gates:
3417 in-language tests (154 files + the 68 corpus cases via `nest test`), 864 Rust
tests, the process/message files under `GC_STRESS`+`GC_VERIFY`+`JIT_VERIFY`,
`nest check` zero warnings, `cargo fmt` clean.

### Two negative results — don't re-chase these

**`nqueens` is not closure-dispatch bound, and JIT-lowering `MakeClosure` would not
help it.** `BROOD_JIT_DUMP_IR` shows `safe?` and the `reduce` step lambda tiering but
**`solve` never lowering** — its `(fn (acc c) …)` is a `MakeClosure`, the exact bail
that was crippling `receive`. That reads like the same bug, and it is not. Rewriting
the port with the `reduce` replaced by an explicit tail loop — no closure anywhere, so
every arm can tier — measured **95 ms, identical to the 95 ms original** (checksum 724
both ways). So the enclosing arm's failure to tier costs nothing here; the 2026-07-24
spike's verdict stands, and admitting `MakeClosure` to the JIT subset should be judged
on its own workload, not on nqueens.

**`loop` is at its floor.** ~40 ms of compute for 30M iterations is ~1.33 ns/iter,
about four cycles for an overflow-checked add, a compare, a branch and the safepoint
tick. Threading the bound as a parameter instead of reading the global (which is how
the Elixir and Node ports are written, so it is *more* faithful, not less) measured
**70 ms vs 50 ms — slower**: the global read is already hoisted, and the third
argument costs more than it saves.

**`bintree` unchanged** and still the one open watch-item — 100% native, `jit_deopt=0`,
its `[left right]` cells escape into `check`, so it remains the boxed-24-byte-`Value`
allocation floor described on 2026-07-24.

### Review addendum, same day — two things the self-review caught

Neither was a correctness break, but both were real and both are now handled.

**1. Expansion got slower, because the macro walked each pattern twice.** The first
cut called `pattern-vars` once to build the matcher's `[idx var…]` and again to build
the call site's rebinding `let`. Measured against a clean pre-change binary built from
`HEAD` in a worktree (a 32-arm `receive`): **2.9 ms → 4.6 ms, +56%**. Fixed by
`receive--prep`, which pairs each clause with its binders and index once and hands the
same prep to both halves: **4.6 → 3.6 ms**, leaving ~+6% over baseline at 32 arms
(~+12% at 16) — the honest cost of the extra expand-time work, paid once per receive
site at load. Growth stays **linear** and there is no cliff at the 13th arm, which is
where KI-10 used to bite (12 arms 1.64 ms, 13 arms 1.86 ms, 16 arms 2.08 ms).

**2. A boot-time regression test quietly stopped firing.** `BROOD_BOOT_TRACE=1` on the
baseline reports `freeze scrubbed 1 dead boot-intermediate closure env(s)`; on the new
build it reports **0**. That scrub was the `offload` wrapper's `receive` — the old
expansion nested a body-thunk `(fn () …)` inside the matcher, so expanding it at boot
left a dead captured-frame closure for `Heap::to_prelude` to scrub, and the prelude
comment called that out as deliberate coverage of the reachability-aware dangling-env
check. ADR-155 removed the thunk, so no prelude form produces one any more and the
**scrub branch is no longer reached at boot**. The invariant is unchanged — the *hard
assert* for reachable env-capturing closures still runs on every closure — but the
incidental exerciser is gone and the prelude comment has been corrected to say so.
Worth a deliberate test if that branch is to stay covered.

**Verified against the pre-change binary side by side** (same machine, same moment;
binaries confirmed distinct): `ring` 1387 → 719 ms, `pingpong` 251 → 195 ms,
`bintree` 118 → 120, `nqueens` 95 → 96, `loop` 51 → 51. Boot unregressed (cold source
boot 47.3 → 45.4 ms; cache hit ~8 ms). `pattern-vars` was also checked to agree with
what `match-compile` actually binds across **all 17 pattern forms** — vector, wildcard,
nested, list-rest `&`, `{:keys …}`, `:or`, quoted, literals, pin, non-linear, guard,
three `(bytes …)` segment shapes, and a 10-binder clause — since a disagreement there
would silently hand a body the wrong variable.
## 2026-07-26 — syntax review, part 2: the collection protocol, and two patterns that lied (ADR-156)

The orthogonality sibling of ADR-154's conciseness pass. Method mattered here: the
matrix of collection ops × collection kinds was probed **against the running
binary**, not read out of `language.md` — which is the only reason the findings are
what they are, because the docs described a coherent protocol and the binary didn't
implement one.

**The set was never wired into the protocol.** `(conj #{1} 2)` raised "not a
collection"; `(into #{} [1 1 2])` returned the list `(1 1 2)`, losing both the kind
and the dedup; `disj` didn't exist in the prelude; and `get` fell through to `nth`,
so `(get #{10 20} 0)` was `20` and `(get #{10 20} 10)` was `nil` — a membership read
answering by position, wrong under any reading, and silent. `first`/`rest` erred on
a **map** while `seq`/`last`/`map`/`filter`/`fold`/`into` all read one as its
`[k v]` pairs.

**ADR-154 had looked straight at `set/conj` and kept it**, on the premise that this
mirrors `clojure.set`. The premise was simply false — Clojure's `conj`/`disj` are
`clojure.core` and polymorphic; `clojure.set` defines neither. Worth noting as a
failure mode: the ADR reasoned from a remembered API rather than checking, and the
"deliberately not changed" framing then protected the mistake. The cost was bigger
than stutter, because a module-local `conj` *shadows* the polymorphic one: any file
with `(:use set)` got "%set-add: expected set, got vector" from `(conj [1 2] 3)`.

**Two patterns lied.** `(match 2 ((or 1 2) :hit) (_ :miss))` answered `:miss` —
`(or 1 2)` is a 3-element list pattern whose head binds a *variable named `or`*. And
a map pattern's unknown keys were ignored, so `{:a v}` degenerated to "is it a
map?": matched anything, bound nothing, then died on an unbound `v` in the body,
pointing nowhere near the pattern. Both are exactly what ADR-152 exists to prevent;
both are now clean errors naming the spelling that works. The rejection lives in
`match-map-vars`/`match-compile-map`, so one edit covers `match`, refutable `let`,
`fn`/`defn` clauses and `receive` — the payoff of one shared pattern grammar.

**`case` existed in the docs, the checker, and nowhere else.** `language.md` said
"`case` is just `match` with literal patterns" (reads as an existence claim), the
runtime carried a foreign-construct hint saying Brood *has* no `case`, and
`check/infer.rs` + `check/walk.rs` both already modelled `(case key v1 r1 … default)`.
It's now a prelude macro over `match*`. Under "one spelling each" an alias wouldn't
qualify; what qualifies it is the **restriction** — a `case` test must be a literal,
and a bare symbol is rejected, because that is precisely where `match` silently
binds instead of comparing. The exhaustiveness lint now reads the embedded context
keyword and names the surface form, so a `case` no longer reports as `match:`.

**Also shipped:** `partial`/`complement`/`constantly` (`comp` had shipped alone,
which left a hand-written `fn` as the only partial application); `vec` (documented
in two places, unbound); `nan?`/`infinite?` — `nan`/`inf` are *reader literals*, so
the language could produce a NaN long before it could test for one; `comment`, with
a `SkipBody` entry so the checker doesn't walk its body; and a printer fix so a
range spliced into a cons tail prints `(9 0 1)` rather than the dotted
`(9 . (0 1))`, a form that didn't read back as its own value.

**Two things the review deliberately did *not* change**, both recorded in ROADMAP
with the reasoning: `contains?` still errors on a vector (Clojure answers by
*index*, making `(contains? [1 2] 1)` true for the wrong reason — a loud error beats
inheriting the trap), and a string is still not seqable (codepoint vs grapheme is
the caller's decision). The remaining six review findings are queued at the top of
ROADMAP, led by the one that needs an ADR before any code: **callable data**
(`(:key m)`), the biggest ergonomic gap left against the Clojure surface.

**Test-writing note worth keeping:** the first version of the map-pattern
regression test used `(macroexpand '(let ({:a v} …) …))` and reported "no error was
raised" — because `let`/`fn` are *special forms* whose pattern lowering runs in the
compile pass, not macroexpansion. Only `match` (a macro) is reachable by
`macroexpand`. For the other three binding positions the probe has to be
`eval-string`. The implementation was right; the test was measuring the wrong thing.

**Suite status.** `nest test --exclude conformance` is 3352 tests / 3350 passing in
30 s, with the 2 failures being one pinned `first` error-text assertion (updated —
the domain now names set and map). The full `nest test` still hits the documented
900 s conformance grind (the 100k-deep JSONTestSuite documents, ROADMAP's one red
item) — unchanged by this work and confirmed pre-existing by running with the tag
excluded.

## 2026-07-26 — `:else` cost 12×, and the fix is a constant-test fold (ADR-157)

Found while sweeping every benchmark row for further headroom after ADR-155.
`ackermann` measured **4286 ms** against the 342 ms in `FRONTIER.md` — a 12× gap that
no commit claimed. It was not a regression in the runtime: the port's `cond` catch-all
is spelled `:else`, and ADR-154 (landed the same day) had stopped special-casing it.

`cond` expands to nested `if`s, so the catch-all became `(if :else x nil)` — an emitted
keyword `Const` plus a branch. That constant is not an integer, so the arm no longer
matched the unboxed-i64 register worker's subset and fell to the general JIT path.
Confirmed with `BROOD_JIT_DUMP_IR`: the `else` spelling produces **no** general-path arm
for `ack` (it is on the register worker); the `:else` spelling produces one.

A/B with the port otherwise untouched, identical checksums: `ackermann` 4285 → 360 ms,
`collatz` 162 → 97, `primes` 96 → 58, `nbody` 330 → 329 (float, never on that path).
ADR-154 had explicitly noted `:else` "still catches" — true, and the reason this hid:
it was never wrong, only slow. Blast radius at the time: `brood-edit` 94 uses, `pong`
40, four benchmark ports.

Fixed in the compiler rather than the callers (ADR-157): `compile_node`'s `if` arm now
folds a literal test to its taken branch. Both branches are compiled first, so slot
allocation and `note_definition` still run and only the losing *Node* is discarded.
General by construction — `else`, `:else`, `true`, `42`, `""` all cost the same nothing
— so no spelling is privileged and nobody has to know the rule. New regression file
`tests/const_test_fold_test.blsp` (6 tests, `:serial` so the folded arms actually tier,
green on VM / tree-walker / no-JIT / GC-stress).

Also refuted the same day, so it is not re-tried: **`send`'s 225 ns fixed cost is not
the registry lock.** Every `send` resolves pid → mailbox through a single global
`Mutex<HashMap>`, which looked like both a per-send cost and a contention point for
`ring`'s 200 processes. Swapping it for an `RwLock` (sends only read) measured
**nothing**: pingpong 199 → 200 ms, ring 721 → 734, the send microbenchmark identical.
The registry is uncontended; the cost is elsewhere in the deliver path. Prototype
reverted.
## 2026-07-26 — the hint table was lying in five places

A follow-on audit of every "the Brood way" hint (`eval::foreign_construct_hint`,
shared by the runtime and `nest check`) against what the tree actually contains.
Five were wrong, and the wrong ones are worse than none: they send the reader
after a feature that isn't there, or deny one that is.

- **`deftype`/`reify` pointed at `defprotocol`/`defimpl` "(the `protocol`
  module)".** There is no such module in std. The macros live in the **hatch
  package** (`hatch/src/protocol.blsp`, whose own docstring calls itself a
  "prototype for std/protocol"), dispatching on the first argument's `type-of`.
  What confused this is that the *kernel* does carry `types/check/protocol.rs` —
  a full conformance pass for `defprotocol`/`defbehaviour`/`defimpl` — plus LSP
  goto/hover, all of it dormant until a project loads a module the tree doesn't
  ship. `docs/types.md` had the same gap and now says so.
- **`letfn` pointed at `let` + `fn`**, which cannot express a recursive local —
  the entire reason `letfn` exists. Now points at `letrec`.
- **`lazy-seq` said "Brood sequences are eager"** with no mention of the
  `lmap`/`lfilter`/`lkeep`/`lremove` fusing seq-views (ADR-111) or the lazy
  `range`. Both have existed for months.
- **The `#` arm claimed `#{…}` was unavailable** ("Brood has no `#` reader
  macros… `#{…}` set literal → `(set […])`") — four ADRs after sets became a
  first-class literal (ADR-060), and with `#b"…"` bytes literals also real.
- **`#_`'s reader hint offered only `;`** — it predated `comment`, which landed
  the same day and is the exact replacement for a form discard.

**And 15 names had no hint at all**: every ADR-154 rename (`car`, `cdr`,
`concat`, `length`, `entries`, `flat-map`, `string-contains?`, `read-file`, …)
gave a bare "unbound symbol" for a name with a one-word replacement. `some?` is
the one that matters most — ADR-154 freed it *because* "any element matches" and
Clojure's non-nil test get confused, so the hint now names both readings.

**Kept as the lesson:** a hint is a claim about the language, and nothing was
verifying those claims. `hints_name_only_features_that_exist` (crates/lisp/tests/
basic.rs) now pins them, including negative assertions — the `deftype` hint must
say "NOT in std" and name `hatch`. Worth extending whenever a hint is added: the
`case` hint (removed earlier today, when `case` stopped being absent) was the
same failure a few hours earlier.

## 2026-07-26 — the syntax review's remainder: protocols, graphemes, patterns, transducers (ADR-158…163)

Cleared every item the review left open. Three became capability, three became
decisions, and one turned out to be already-built-but-unshipped.

**Protocols were 90% done and 0% shipped (ADR-158).** The hint that started this said
*"use `defprotocol`/`defimpl` (the `protocol` module)"* and no such module existed.
Chasing that found the kernel carrying `types/check/protocol.rs` — a complete
conformance pass — plus LSP goto/hover, dormant for months, while the macros lived in
the **hatch package**, in a file whose own docstring says *"prototype for
std/protocol"*. So the design was built, proven in a real app (hatch's JSON `Encode`
replaced a closed `cond` on `type-of`), and checker-validated; nobody had promoted it.
Promoted verbatim; hatch's copy deleted; hatch's 750 tests pass against std's. The
lesson: a hint that names a nonexistent feature can mean the feature is *finished
somewhere else*, not that it was never designed.

**Grapheme-indexed accessors (ADR-159).** `language.md` has said for months that a
cursor must step by cluster, while every indexed op is codepoint-indexed — so the
correct spelling was `(nth (string->graphemes s) i)`, segmenting the whole string per
keystroke. `grapheme-count` / `grapheme-at` / `substring-graphemes` walk to the index
instead. Every test uses a **decomposed** `e` + U+0301: a precomposed U+00E9 makes the
tests pass while proving nothing, which is the trap in this area.

**`or`/`and`/map sub-patterns implemented (ADR-160).** ADR-156 had made all three loud
errors that morning; making them work was the natural follow-through, and `and`
doubles as the `:as` capture (`(and whole {:keys [a]})`) so no `:as` is needed. Two
design points: an `or`'s alternatives must bind the **same names** (else the body
references a name whose existence depends on the input — Rust's rule, same reason),
and `success` is **duplicated per alternative** rather than thunked, because a thunk
would cost the tail-position guarantee. Explicit map keys **require presence** while
`:keys` stays lenient — Erlang semantics for the map-pattern spelling, Clojure
semantics for the destructuring spelling, both on purpose.

**Transducers made public (ADR-161).** The stage constructors had been private since
ADR-111, which meant a user could not write a stage of their own. The contract is two
sentences, so publishing it costs a paragraph: `transduce` + `xmap`/`xfilter`/
`xremove`/`xkeep`, and a custom stage is a plain `fn`.

**`lambda` retired (ADR-162).** ADR-098 said drop the aliases, `let*` went, `lambda`
stayed, ADR-108 then said the opposite, and `language.md` claimed for months that it
was gone — the docs sweep earlier today even removed mentions *on that basis*. Zero
uses in `std/`, zero across 12 siblings, zero in tests outside the file testing the
alias itself. Gone; the unbound-symbol hint names `fn`. One diagnostic changed with
it: an inline callback of the wrong arity was called "the lambda", naming a form the
language no longer has.

**The convention questions became one ADR of decisions (ADR-163)**, not code: no
`&key` (a trailing options map is the rule, and the migration sweep turned out empty —
nothing in `std/` takes more than two `&optional`s); `fold`+`reduce` both stay with the
relationship documented (renaming `fold` would be a 200-site rename of an ambiguous
name, and ADR-154's sweep documented how that goes wrong); bare `else` stays; `!`'s
three meanings documented rather than unified; naming lineage is "best name for the
job" plus `apropos`; failure is throw-for-bugs / tagged-value-for-expected; the reader
gaps get documentation. Also landed: `dissoc-in`, and `for` finally takes multiple body
forms like every other iteration form.

**A real property found by the parallel suite.** The protocol registries are updated
with `def` — a read-modify-write of an immutable map — so *concurrent* `defimpl` calls
can lose an update. A test registering impls inside parallel test bodies passed
standalone and failed in the full run. That is the honest contract (register at load
time, exactly like telemetry's `attach`), so it is now in the module docstring, in
`language.md`, and the registering tests are `:serial`. Worth remembering as a pattern:
"passes alone, fails in the suite" on a module with a global registry is almost always
this.

**Gates:** 3426 in-language tests, 861 Rust tests, `nest check` at zero warnings.
The 6 `nest::registry` tests fail for an environment reason unrelated to any of this —
`commit.gpgsign` + 1Password's `op-ssh-sign` makes `git commit` hang in the temp repos
they build; a bare `git init && git commit` outside the repo reproduces it.

### Follow-up, same day — verifying ADR-155/157 against the day's later commits

`2455335` (hint audit) and `d945d3d` (protocols, grapheme indexing, or/and patterns,
transducers — ADR-158…163) both landed after ADR-157. Two checks mattered and both came
back clean, plus one correction to ADR-157 and one CI fix.

**or/and patterns vs the `receive` protocol — the risk, checked.** ADR-155 has the
`receive` macro derive a clause's binders from `pattern-vars` and unpack them
positionally at the call site, so any pattern form where `pattern-vars` disagrees with
what `match-compile` binds would silently hand a body the wrong variable. `d945d3d` adds
two new pattern forms, so this was the exact shape of a latent break. It extended
`pattern-vars` correctly — an or-pattern reports its first alternative's vars (all
alternatives are forced to bind the same set by `match-or-check`), an and-pattern the
union — and all five or/and shapes round-trip through a `receive` clause: first
alternative, second alternative, and-pattern, or-with-guard, and an or nested inside a
vector. The 17-form pattern matrix still passes too.

**Coverage denominators DO move under ADR-157 — the ADR overclaimed.** It said "nothing
about diagnostics or LSP nav moves", which is true but incomplete. `RecordLine` is
emitted per positioned node in `emit.rs`, so a folded-away branch never gets one. A/B on
a fixture (temporarily gating the fold to get the before number): `(if false (+ y 999)
(- y 1))` reports **33% of 3 executable lines with the fold off, 50% of 2 with it on**.
Judged an improvement and kept — an unreachable branch is not "uncovered", and counting
it makes 100% unreachable for no actionable reason — but it is now recorded in ADR-157,
since a project's percentage can move with no test changing.

**rustfmt was red on main again.** `2455335` and `d945d3d` between them left four
unformatted spots (`eval/mod.rs`'s two long hint arms, a `types/check/tests.rs`
assertion, a `basic.rs` `run(…)` call). Same failure as `5f3d145` earlier today, and the
same fix. Worth noting the pattern: three of the day's commits shipped with
`cargo fmt --all --check` failing, so the dedicated rustfmt CI job would have been red on
main each time.

Merged-tree gates after all of it: **3428 in-language + 68 corpus tests, 865 Rust tests**,
`nest check` zero warnings, fmt clean.

## 2026-07-27 — documentation run: `ability` reaches the docs, and validation found four real bugs

A full sweep of `docs/` against the live tree, prompted by `b1989ff` retiring
`defprotocol`/`defimpl`. The retirement itself shipped with **no documentation
change at all**, so the language reference still taught a facility that no longer
exists — and validating the rest of the docs mechanically turned up four defects
that were nothing to do with abilities — including a **red test on main**.

**`ability` was documented in exactly one place — the design note.** Grepping
`defability|ability.blsp` across every doc + README + ROADMAP returned a single
file, `protocol-dispatch-design.md`, while `language.md` §Polymorphism still
presented `defprotocol`/`defimpl` as the live answer (complete with the
now-false "records dispatch as `:map`, branch on a field inside"). Rewrote that
section around abilities: the two dispatch identities, `defrecord*`, `:sealed`,
drivers-as-values, `satisfies?`/`record?`/`record-id`/`fields`, and a
`defbehaviour` subsection for the module-contract seam that stays. Added an
`ability` section to `brood-for-claude.md` (the AI pocket reference had none),
`std/ability.blsp` + a rewritten `std/protocol.blsp` row to the module table, and
fixed the README's three protocol claims. Recorded the decision as **ADR-168** —
the retirement had no ADR, and it settles a *pre-1.0 breaking* question, so it
needed one. `roadmap-for-v1.md` §3 ("decide the protocol `:type` dispatch axis —
permanently") is now closed by it: the `:type`-field axis is permanently rejected,
and `defrecord*`'s construction-time identity is the explicit version that
replaces it. Two of that file's three pre-freeze items are now done, leaving only
the reader's permanent reservations.

**Bug 1 — `impl` silently misregisters a bare record id (KI-15).** Found by
following the `impl` docstring verbatim: `defability`'s `:sealed [circle rect]`
qualifies a bare symbol against `(current-ns)`, but `impl`'s `id-kw` is a plain
`(keyword (name key))`. Since `identity-of` yields `:geometry/circle`, a bare
`(impl Shape circle …)` registers under `:circle` — a key no value ever presents.
No error at registration; the sealed check reports the member unimplemented and
the call dies with `no impl for :geometry/circle — have (:circle)`. Documented the
qualified form as required and filed the asymmetry rather than changing dispatch
semantics mid-docs-run.

**Bug 2 — main was red, and the red test was itself a stale doc.** `b1989ff` rewrote
the `deftype` hint to point at `defability`/`impl` but did not update
`basic.rs::hints_name_only_features_that_exist`, which still asserts the hint
contains `(require 'protocol)` and `defprotocol`. So `make test` reports **879
passed, 1 failed, 1 timed out** — the failure being that assertion, the timeout
being KI-14. The irony is exact: the test exists to enforce that "a hint must never
name a feature the tree doesn't have," and its own assertion had become the thing
naming a retired feature. Repointed at `(require 'ability)`/`defability` and
recorded the episode in the test's doc comment, which already carries this history.

**Bug 3 — the LSP was never migrated off the retired forms (KI-16).**
`definition.rs`/`module_ref.rs` still match `"defbehaviour" | "defprotocol"` (dead
arm), and `completion.rs` offers ops inside `(defimpl …)` — so op completion inside
an `(impl …)` form is simply missing. `defbehaviour` goto/hover is unaffected.

**Bug 4 — `packages.md` documented a primitive that does not exist.** It named
`%sha256` in five places as "the **only** hashing primitive", with a code example
built on it. The real primitive is `%digest` (with `%hmac`); `std/hash.blsp` is
Brood over it and the package manager uses `hash/sha256-bytes` + `slurp-bytes`.
Replaced the section with the shipped implementation — which also retired the
doc's own caveat ("source files are UTF-8, so `slurp`-as-string is exact for v1; a
binary dep would want a bytes read"), since the bytes read already landed.

**Bug 5 — eight doc examples could not run, all the same bug.** Automated the check
(extract every `lisp`/`clojure` block, map each `(require 'M)` against `M`'s actual
exports from the live image, flag bare use of an exported name) — post-ADR-065 a
bare `require` only *loads*, so every one of these was broken as written:
`language.md`'s io-ports, `proc/gen`, `log` and `telemetry` examples,
`building-an-editor.md` ×4 (one also had an unquoted `(require render)`), and
`testing.md`'s headline example — which is the worst of them, since CLAUDE.md
already documents this exact trap for `test`. All now open with `(:use …)` in a
`defmodule` header; the scanner reports zero remaining.

**Mechanical validation, and what it found.** Beyond the above:

- **`primitives.md` claimed to be "the complete set of functions implemented in
  Rust" and was missing 149 of them** — bitwise, transcendental math, decimals,
  sets, Unicode/normalization, TCP/TLS, subprocess, crypto, git/archives,
  coverage, GUI, audio, clipboard, scheduler/profiling, GC/VM stats, plus ~50 that
  belonged in existing categories. Generated the rows from `PRIMITIVE_DOCS`
  (name + arg list → arity + purpose) rather than by hand, so they match the
  source. The completeness claim is now true, and it *stays* true by the existing
  drift-guard test (`every_user_facing_primitive_is_documented_and_no_orphan_docs`,
  verified passing) — every user-facing native must have a `PRIMITIVE_DOCS` entry,
  and every entry now appears in the doc.
- **`primitives.md` still said `catch` binds a message string.** It binds a
  structured map (`:kind :message :code :file :line :col :hint`) for a kernel error
  and the exact thrown value for a user `throw`. The stale text even called the
  structured version a future refinement "once map literals exist". Rewrote it
  against `error-codes.md`, with a runnable `:code`-branching example. Its
  `LispError` sketch was also two fields out of date.
- **`spawn` was listed as a native primitive** with arity ≥1. The primitive is
  `%spawn` (arity 1, takes a thunk); `spawn` is the prelude macro. Fixed, and added
  the missing `%spawn-link`.
- **Env flags: the docs and the source had drifted both ways.** Every flag in
  CLAUDE.md's table does exist (checked all 24). But 11 more exist in the tree and
  were undocumented — `BROOD_DUMP_CODE`, `BROOD_LINMAP`, `BROOD_NO_JIT_COMPUTED`,
  `BROOD_NO_HANDOFF`, `BROOD_DBG_CONST`, the GUI/audio runtime selectors, and four
  implemented in *Brood* rather than Rust (`BROOD_CONTRACTS`,
  `BROOD_NO_CHECK_CACHE`, `BROOD_TEST_NO_SCOPE`, `BROOD_HISTORY`) — added with
  their real semantics read off the implementations. The seven flags that appear
  only in devlog/decisions are historical and correctly absent from the table.
- **Four `std` modules were absent from the module table** — `file`, `io`, `text`,
  `ansi`. Wrote the rows from their actual exports (`io` is the *ports* toolkit;
  `text` exports exactly `fill`; root `ansi` *strips* escapes where
  `editor/ansi` *emits* them — worth disambiguating).
- **The ADR index stopped at 129**, missing 130–167 despite the file's own promise
  that the index keeps the numbering complete. Backfilled all 38 from the headings,
  marked 158 superseded, added 168.
- **Broken links and stale paths.** `concurrency-v2.md`, `supervision.md` and
  `docs/README.md` were deleted or moved (the first two in `fdce540`, which claimed
  to repoint every inbound link and missed three); `llm-native.md` pointed at
  `docs/prompts/system.md` where the shipped file is `brood-task.md`;
  `std/agent.blsp` → `std/proc/agent.blsp` and `std/reload.blsp` →
  `std/tool/reload.blsp`. Verified the remaining ~50 "missing" paths are all
  legitimate — proposed work items (`components.md`'s W2 `core/env.rs`),
  hypotheticals (`node-connect.md` arguing *against* a `std/node.blsp`), install
  paths, and scaffold placeholders.
- **ADR cross-references are sound.** All 168 numbers cited across docs, `crates/`
  and `std/` resolve; no duplicates; the four gaps (002/035/039/057) are the
  archived ones and `decisions.md` says so.
- **186 code blocks parsed; one didn't** — an elided map literal in `layers.md`
  (`{… :type :magit-status …}`, odd form count). Fixed.
- **The formal spec's normative table used Clojure brackets.** `spec.md` §7.1
  documented `(fn [params] …)`, `(let [n₁ v₁ …] …)`, `(letrec […] …)` and
  `(defmacro name [params] …)` — all four are **hard errors** in Brood
  (`fn: parameter list must be a list, not a vector`). The worst placement possible
  for that mistake: it contradicts ADR-149 and ADR-010's "code is lists, data is
  vectors", in the one document that claims to define the language. Every other doc
  had it right — `language.md` even tabulates the bracket form *as* an error with
  its hint. Fixed, with the ADR-149 rule stated inline so the table can't drift back.
- **`spec.md` §4's value model was missing 7 of 19 kinds** — `set`, `bytes`,
  `decimal`, `rope`, `pid`, `ref`, `table` — while asserting "a value is exactly
  one of" the twelve it listed. The `table` omission made the section
  self-contradictory: the paragraph immediately below claimed every value is
  immutable with "no atoms or cells", where `Table` is precisely the one
  identity-mutable kind (ADR-107). Added the kinds and gave immutability its
  one-exception carve-out, including *why* it's compatible with share-nothing (it
  deep-clones in and out, so no two processes alias stored data). Also: `integer`
  said "64-bit signed" with no mention of **bignum promotion** — `(* i64::MAX 2)`
  is exact, so the type is unbounded in practice (`language.md` had this right).
- **`spec.md` §5's evaluation rules predated callable keywords and set literals.**
  Rule 4 said `h` evaluates to "a function" and "applying a non-function raises",
  which is now wrong: a keyword is callable as an accessor (ADR-165), while map,
  vector and set deliberately are *not*. Rule 3 covered vector literals but not map
  or set literals (both evaluate their forms left to right — verified). And §11
  still listed "a first-class set type + `#{…}` literal" as **deferred** when
  ADR-060 shipped it; `type-of` reports `:set`.

Also refreshed the now-historical framing of `protocol-dispatch-design.md`: the
status block said "Slices 1–2 shipped" while the body described 3 and 4 as done and
listed "retire/migrate `protocol`" as open. It is now marked resolved, with a note
recording *which* fork was taken and why — the registry route, not the Go-style
structural satisfaction the note had been leaning toward, because losing
retroactive extension of a foreign record was the larger cost.

Also fixed **11 stale `std/` paths in Rust doc comments** (`std/repl.blsp` →
`std/tool/repl.blsp`, `std/http.blsp` → `std/net/http.blsp`, and so on) — two of
which reach users directly: `nest --help`'s `-j` text and the `mailbox-size` /
`process-info` docstrings that `(doc …)`, the LSP and MCP all surface.

Every example quoted in the rewritten sections was executed against
`target/release/brood` before being committed — including the claims that a record
is `≠` a bare map, that `keys` includes `:__id__` while `fields` doesn't, and that a
plain map carrying `:type` still dispatches as `:map`.

**Gates.** `nest check` zero warnings; `cargo fmt --all --check` clean; **880 of 881
tests pass** — the one remaining failure is KI-14's `brood_suite_passes` timeout,
which was already open and is unrelated to this work. Before this run it was 879,
because of Bug 2. No behaviour changed: every edit is documentation, a doc comment,
or one test assertion repointed at the hint it is supposed to be pinning.
## 2026-07-27 — KI-14 root-caused and fixed: `make test` goes green

**KI-14 was not what it looked like.** Filed as "one conformance test hangs, but only
under the test framework", with a note that it was *not* deep recursion (the deep
documents parse fine in a spawned process). Both halves of that framing were wrong in a
way that mattered: the framework was irrelevant except for **how much code it had
loaded**, and deep recursion was the trigger — just not in the parser. Full write-up in
[known-issues.md](known-issues.md#ki-14).

The hang is in the **ADR-091 RUNTIME collector's drain report**. Its two-phase liveness
probe throttles Phase 2 (the O(heap) walk) but runs Phase 1 unconditionally, on the
premise that Phase 1 is cheap. Phase 1 seeds from `roots` — the VM operand/env stack —
so its cost is **O(recursion depth)**. Instrumenting the hung run, inside a *single* drain
epoch: **78 409 Phase-1 walks over a 1.7-million-entry root stack.**

Three compounding faults, in order of how much they cost:

1. The Phase-2 stale-dirty short-circuit sat **between** the phases, so a Phase-2-dirty
   process paid a full Phase-1 walk every safepoint and discarded the result. Hoisting it
   changes no verdict — it only skips work that was already being thrown away.
2. Phase 1 had no throttle of its own. Added `P1_REVALIDATE_STRIDE`, gated on
   `P1_LARGE_SEED` so shallow processes keep their every-safepoint promptness.
3. `live_vm_arms` is a per-frame stack, so a recursive function appears once per frame
   (66 448 entries, a handful of distinct `Arc`s) and each arm's whole IR tree was
   re-walked per entry. Deduped by `Arc` identity.

Filed repro: hang → **9.8 s**. Full in-language suite including conformance: **3591 tests,
all passing, 88 s** — the gate that could not go green.

**Two real stack-overflow aborts found alongside it, both distinct from the hang.** Worth
separating clearly, because the first one initially looked like the answer and is not:

- **JIT arms could run the native stack into its guard page.** `jit_native_depth` + the
  `stacker` probe guard the *dispatch* paths only; recursion through `brood_rt_call_slow`
  re-enters Rust every level while the counter stays near zero, so neither cap ever fired.
  Now every lowered arm's prologue compares its own frame address against a
  `Heap::jit_stack_limit` stamped from the live remaining stack at each native entry
  (three instructions), sets `jit_force_vm` and deopts on a trip. Deopt alone livelocks —
  VM re-runs the arm, callee re-tiers, prologue trips again — which cost an hour before
  the flag went in. Guarded by `tests/jit_deep_recursion_test.blsp`; verified it aborts
  with the fix stashed.
- **`gc_runtime::flush_rt_value` ⇄ `flush_rt_pair` recursed unguarded.** The cdr spine was
  made iterative by an earlier fix; *car* nesting still recursed a native frame per level.
  The LOCAL twin `gc::flush_value` already had `stacker::maybe_grow` — the RUNTIME one was
  simply missed.

**Swept for the same defect class** (recursive native walker over user-controlled depth,
where the reader's 256-level cap doesn't apply because the value is built at runtime).
Probed print, equality, hashing, compare, `send`, table put/get/snapshot, `def`/promote,
GC, `macroexpand`, `eval`, sort, and map/set keys against 200 000-deep lists, vectors and
maps. All bound cleanly except **`value_cmp`**, the ordering sibling of `equal` and
`hash_value_into`: both of those grow the stack, it recursed raw and aborted. Fixed the
same way, and `tests/deep_values_test.blsp` — which already pinned promote, GC, equality
and hashing — now covers ordering and `sort` too.

**Overlap with the documentation run above.** Both sessions touched
`hints_name_only_features_that_exist`. That entry repointed the `deftype` hint at
`defability` and fixed the test it had left red; this one carried it one step further —
the hint said `(require 'ability)` "gives" `defability`, but `require` only *loads*, so
the name stays qualified (`ability/defability`). The hint and its assertion now say
`(:use ability)`, and the test checks that against the live image (`bound? 'defability`
is `false` after a bare `require`) rather than only matching the wording.

**Method note.** gdb cannot *attach* here (yama `ptrace_scope`), which is what stalled the
original investigation. It can still *launch*: `gdb --batch -ex run -ex "thread apply all
bt"` plus a background `pkill -INT` to sample a running hang. Four samples put every hit in
`seed_phase1_and_walk`; a temporary `eprintln` counter turned that into the exact numbers
above. Two wrong theories died on those measurements (image size, then epoch count) — the
counter is what ended the guessing.

## 2026-07-27 — published benchmark run: KI-14's stack guard costs a call

Full cross-language run against `1a3fc1c` (`brood-benchmarks` `c755975`), published.
**Ranks are unchanged on all 28 rows and no row is last-of-seven**; the aggregate reads
3.2× of .NET (was 3.0×), still 3rd of seven ahead of Elixir. Most rows drifted +2–5% in a
field where every language's figure rose, i.e. machine drift, not a code change.

**Two rows moved for a real reason, and it is the KI-14 fix above.** `fib` 57 → 75 ms and
`pfib` 165 → 218 ms — the same ~+30%, seen once and then ×100. Bisected with three
separately-built binaries (`26939e2` pre-guard / `f11f4cb` guard / `1a3fc1c` HEAD, identical
`release-fast` + feature flags, best-of-7 wall each, `taskset`-pinned):

| binary | `fib` wall | `pfib` wall |
|---|---|---|
| `26939e2` (pre-guard) | 70.1 ms | 182.8 ms |
| `f11f4cb` (guard) | 85.6 ms | 239.0 ms |
| `1a3fc1c` (HEAD) | 85.3 ms | 236.6 ms |

So the whole move lands on `f11f4cb` and nothing after it contributes. That is the right
trade — an abort that took the OS process and that `try`/`catch` could never see became a
catchable error — and it was made with the perf cost unmeasured, which this run supplies.

**The cost is probably not irreducible, and the suspect is named.** The prologue check
itself is three instructions against a preloaded absolute address. But `stamp_stack_limit`
calls `stacker::remaining_stack()` on **every** `jit_run_fast_link` — the ~26 ns
Brood→Brood path that `fib` is made of. The limit is an absolute address valid for the
whole thread stack, so re-deriving it per link is redundant work: stamping only at the
outermost native entry (`jit_native_depth == 0`) plus wherever a green process is
(re)scheduled onto a worker (stack bases differ, which is why the stamp is live rather than
constant) should keep the guarantee while taking the probe off the hot path. Unmeasured;
recorded as lever 0 in the benchmark repo's `FRONTIER.md`.
`tests/jit_deep_recursion_test.blsp` aborts the process if the guard regresses, so the
experiment checks itself.

**Also re-measured, and better than the note it replaces:** `sort` read 194 ms in-suite.
The 2026-07-26 table carried a hand-patched 188 ms from an isolated re-run (its full-run
sample was 208 ms) — this run measures the row directly, so the footnote is gone. Base RSS
is 20.2 MB: 3rd rather than 2nd, having drifted ~1 MB past Ruby's 19.2 MB, still the
lightest of the compiled-class runtimes.

## 2026-07-27 — reader reservations (ADR-169): the last pre-freeze language decision

Closed `roadmap-for-v1.md` §2 — the reader's permanent reservations, the one open
pre-freeze **language-surface** decision. The reader stopped spending two token spaces
by accident, stated as one rule on a token's *first* character:

- **`#` is a dispatch character, not an atom character.** `#{…}` (set) and `#b"…"`
  (bytes) are the only `#` forms; every other `#…` — `#foo`, a bare trailing `#`, and
  `#|…|#` (a Scheme/CL block comment, which had read as the bar-quoted symbol
  `|#\|…\|#|`) — is a reader error with a teaching hint. A trailing `#` *inside* a token
  (`x#`) stays quasiquote auto-gensym.
- **A digit-led token must be a number.** A token leading with a digit, or a sign/dot
  then a digit, that isn't a number Brood has (`1/2`, `0x1F`, `1_000`, `1N`, `1+`,
  `12-34`) is a reader error — it used to intern as a symbol and resurface far away as
  "unbound symbol". New `AtomKind::ReservedNumeric` + `digit_led`, with a shape-specific
  hint (`reserved_numeric_hint`) shared by the reader and the tooling CST, and the kind
  maps to `NodeKind::Error` so the LSP flags it like a malformed literal. Names with a
  sign/dot but no digit behind (`+`, `-`, `...`, `.foo`, `--5`) are untouched.

The point is the freeze asymmetry (same as ADR-166): *relaxing* a reservation later is
backward-compatible, *adding* one is not — so a freeze has to reserve first. The cost is
nil (no real program names anything `#foo`/`0x1F`; `inc`/`dec` are the Brood spelling of
`1+`/`1-`; Clojure rejects the same tokens), and it keeps every future numeric syntax
(ratios, radix literals, digit separators, a bigint suffix) and every future `#` literal
purely additive. Ratios are now a documented "not in 1.0" with the `1/2` token reserved,
so a later ratio type is additive rather than breaking. The printer needed no change —
`symbol_needs_bars` asks `atom::classify`, so `(symbol "1+")` began bar-quoting to `|1+|`
on its own (the ADR-025 one-definition rule).

Docs: ADR-169; `roadmap-for-v1.md` §2 marked done + three freeze-list rows; `language.md`
data-type table (Symbol row rewritten, `#|…|#` and digit-led rows added); the ROADMAP
`#|` item closed. Tests (committed with the code, `e83affe`): `reader_hints_test.blsp`,
`reader_malformed_test.blsp`, `malformed_test.blsp` — 20/20/72 green, build clean. **All
four pre-freeze language-surface items (ADR-165/166/168/169) are now done; the surface is
freeze-ready** — what remains is ratifying the freeze list as its own ADR, plus the
non-language release blockers (`nest format --check`, the registry-test signing-agent
coupling).

## 2026-07-27 (later) — the KI-14 guard cost, recovered: it was the second compare

Follow-up to this morning's entry, which named `stamp_stack_limit`'s per-fast-link
`stacker::remaining_stack()` probe as the suspect for `fib` 57 → 75 ms. **That was wrong, and
the fix is elsewhere.** `fib` is now 74 ms against 88 for HEAD in a same-session best-of-7 A/B
(`pfib` 252 → 202), which is parity with a build that has the guard deleted outright (73 / 199).

**Hoisting the stamp measured exactly zero.** The hoist itself is sound and shipped — stamp at
the outermost native entry (`jit_native_depth == 0`) plus at quantum start in `Process::drive`,
since a green process resumes on whichever worker the scheduler routed it to and worker stack
bases differ — but `fib` never executes it. `fib` tiers to the **i64 register worker**, which
recurses natively through its own body; a fast link is not involved. Rebuilding with both KI-14
prologue guards deleted recovered the whole 16 ms, which put the cost on the per-frame prologue
and nowhere near the stamp.

**Attributing inside the prologue, one binary per hypothesis** (fib, best-of-7, taskset-pinned):

| build | `fib` | note |
|---|---|---|
| HEAD `2e5346c` | 89 ms | both guards as shipped |
| `limit != 0` test dropped | 84 ms | the compare is unsigned — no address is `< 0`, so a `0` limit already disables the check for free |
| frame-address probe dropped | 84 ms | ~0 |
| `limit` load dropped | 82 ms | ~2 ms |
| **frame-count cap dropped** | **73 ms** | full recovery |
| both guards deleted | 73 ms | the floor |

So it was never one expensive instruction — it was **two tests per level where one would do**.
The byte guard ran alongside the old `I64_DEPTH_LIMIT` frame-count cap and `bor`'d the results,
and a second compare on every level of a ~30 M-call recursion is ~18% of the row. The count cap
is now gone: the byte test measures the resource that actually runs out and subsumes it, and a
frame count is only ever right for one frame size — which is precisely what KI-14 was (the JSON
document whose frames were heavy enough that 32 768 of them exhausted 16 MiB long before the
count tripped).

**The one thing the count cap still covered** is a `0` limit, i.e. a platform whose remaining
stack can't be read: the unsigned compare then never trips and the guard fails open, which with
no count behind it would be an unguarded native recursion. The i64 **wrapper** now checks the
limit once per outermost activation and returns outcome 5 when it is `0` — the same
"this fn belongs on the boxed path" signal the depth-bail raises, so `jit_tier` retires the
register version and the boxed path (which keeps its dispatch-level depth caps) takes over.

**The stamp hoist was kept on its own merits**, measured separately after the fact: ~5% on
`bintree` (130 → 124 ms, best-of-15 — the 16.3 M-fast-link row), flat on `nqueens`/`json`/`sort`
and on `fib`. Two stamp points, and the second is not optional: without the quantum-start stamp,
a process that migrated to another worker while its depth counter was non-zero would keep
comparing against the previous worker's stack.

Guard integrity is unchanged: `tests/jit_deep_recursion_test.blsp` passes at 2.77 s vs 2.63 s for
HEAD (same nest profile — an earlier 14.5 s reading was a `release-fast`/`--no-default-features`
nest, not a regression).

## 2026-07-27 — type-checker gate cleanup (93 → 8 gating) + ADR-170 freeze list

Drove the `nest check std/ tests/` gate — deliberately red since 2026-07-25 (81
warnings, per the `ci.yml` note) and drifted to 93 — down to 8, all remaining ones
checker false-positives or intentional macro anaphora. In order:

- **Two real defects the gate had been masking:** `pad-left`/`pad-right` gained an
  optional pad-char arg (json.blsp was calling the 2-arg form with 3 to zero-pad a hex
  escape); `log/file-backend` called a non-existent `file/append-file` where the
  `spit-append` primitive was meant.
- **Three unbounded-data helpers made properly tail-recursive** (`insert-by-ms`,
  `pq--sorted-insert`, `multimap--remove-first`) — they recursed to the length of a
  caller-controlled list, so a large input could overflow; now `letrec` + reversed-prefix
  accumulator.
- **The non-tail-recursion lint is now advisory, not gating** (Option A). Its premise —
  a *silent* stack overflow — expired when deep recursion became a catchable
  `MAX_BC_FRAMES` error; and it fires on correct code it can't rewrite (tree recursion,
  bounded compile-time helpers). `nest check` prints those with an `(advisory — not
  gating)` marker and excludes them from the exit code (`project--tally-warning`), which
  reclassified 53 warnings. `check-allow :non-tail-recursion` still works for a targeted
  opt-out; a structured warning-severity tier is the fuller fix, deferred.
- **The stale suite-failures demo** was modernized to a `(:use test)` module — it had
  silently stopped running post-ADR-065 (bare `(require 'test)` no longer imports names)
  and read as 20 `unbound symbol` warnings.
- **Four unused `:use` imports removed** (http/net-tcp, wasm/file, ui/editor-face,
  project/format).
- **Two checker false positives fixed:** `%node-connect`'s registered signature said arg
  1 was `symbol` and the return `symbol`, but `expect_node_name` accepts keyword-or-symbol
  and the fn returns `Value::keyword` → now `(sym|keyword, string) -> keyword`, matching
  register/whereis/monitor-node; and the call-arity + arg-type checks read the *global*
  arity/sig even when the head is a lexical local shadowing a builtin (`(let (exit …)
  (exit model))`), now guarded by `is_lexical_local` like the declared-sig lookup already
  was.

Remaining 8 are all checker-side: the hygiene lint misreads `as->`'s computed `~steps`
splice as "binds `unquote`"; `defseq`'s `item` is intentional anaphora the lint doesn't
model; `(rest x)` on a dotted `(name . ms)` pair is typed as a list (three `>=`/`>` sites);
and an untyped `fold` `coll` param widens to `any`. All checker fixes, not code defects.

**ADR-170 ratifies the 1.0 freeze list** — the permanent record of what Brood refuses
(mutation, `while`/`loop`/`recur`, `&key`, ratios, nominal types, multiple dispatch,
monkey-patching, …), generated by ADR-011 + the freeze asymmetry, each row citing its
own ADR. `roadmap-for-v1.md`'s freeze-list section now points at it as the ratified
source.

**Not done: the tree-walker differential CI job.** Both its failures
(`vm_tail_arm_compaction`, `std_attribution`) *pass under the VM* and fail only under
`BROOD_VM=0`, and its 6 timeouts are the tree-walker running the full suite ~10× slower
than the time budget. So it is a CI-job-health issue (engine-specific unit tests running
under the wrong engine + tree-walker slowness), not a GC bug — left for a deliberate pass.

## 2026-07-27 (later still) — `reverse-onto`: naming the shape every tail rewrite lands in

The non-tail-recursion advice (`508445a`) pushes std toward accumulator loops, and each
rewrite ends the same way: walk a list building a reversed prefix, then splice it back in
front of the remainder. Written the obvious way — `(append (reverse acc) tail)` — that is
**four passes**: `reverse` walks `acc`, then `append` folds both lists onto nil and reverses
the result. The one-pass loop already existed as the private `reverse--acc`, and the
prelude's own `merge--acc` carried a comment explaining why it used the fast form — but it
had no public name, so `5b40e7b`'s three new rewrites (plus `queue`, `multimap`, `url`,
`tool/test`) all reached for the slow spelling.

So the idiom gets a name. Measured, 20k splices of a 500-element prefix:

| form | time |
|---|---|
| `(append (reverse acc) tail)` | 2739 ms |
| `(fold flip-cons tail acc)` | 1674 ms |
| **`reverse-onto`** | **601 ms (4.6×)** |

The `fold` form loses on the per-element `flip-cons` apply frame, which is exactly the
reason `reverse--acc` was written as a dedicated loop in the first place. Applied at six
call sites plus `append--onto` (so `append`/`mapcat` inherit it) and `merge--acc`.

**A measurement that nearly went in wrong.** The first comparison built the accumulator with
`(range 0 500)` — so `fold` hit `%range-reduce`, the kernel's counted-loop fast path, and
came out *ahead* of the dedicated loop (1050 vs 1488 ms). With genuine consed lists, the
shape the real call sites have, the ranking inverts. A lazy range is not a list, and a
microbenchmark that hands one to `fold` is measuring the wrong function.

**And one that did not survive scrutiny — recorded so it isn't re-found and believed.** The
suite A/B read `wordcount` −5.8% and `bintree` −4.8%, solo-confirmed both. Neither program
calls `append`, `mapcat`, `sort`, `reverse` or any module touched here, so there is no causal
path. A definition-only control (the new `defn` present but unused) measured identical to
upstream, and adding a further irrelevant `defn` did not move the changed build, so it is not
alignment luck either. What settles it: the gain **shrinks with workload** — bintree −10.7%
at N=50, −9.0% at N=100, −4.8% at N=200, −1.8% at N=400 — i.e. a fixed ~5 ms, not a
per-operation win, while `fib`/`json`/`sort` are flat. Real, reproducible, unexplained, and
**not** claimed as a benefit of this change. `sort` in particular was predicted to improve
via `merge--acc` and measured +1.0%: its splices are short next to the row's real cost, which
is building the input list.


## 2026-07-28 — tree-walker use-after-GC on `(runtime-collect)` in an `&optional` default

Found chasing the tree-walker differential CI job: `vm_tail_arm_compaction` passed under
the VM but crashed under `BROOD_VM=0` with `expect("runtime closure handle")`. Not a VM
regression — a real tree-walker use-after-GC, pre-dating KI-14, that only the test's lean
`Interp::new()` slab layout triggered (the CLI, with more RUNTIME code loaded, put the
template at a safe index and never reproduced it).

The backtrace named `eval::record_tw_entry`: the tree-walker's error-trace bookkeeping
re-read `heap.closure(id).name` *after* `bind_params`, but `bind_params` evaluates the
`&optional` default `(do (runtime-collect) 0)`, which compacts the RUNTIME closures slab
and invalidates `id`'s index. The VM's `apply_closure` already avoided this by capturing
the name up front (kernel-audit #2); the `eval_tail_loop` path didn't. The name was in
fact already captured one line earlier (`cl_name`, for the arity-error trace), so
`record_tw_entry` now takes that pre-captured `Option<Symbol>` (interned `u32`, GC-stable)
instead of re-dereferencing the closure. Fixed on both engines under
`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`.

Audited the sibling derefs for the same class — every other `heap.closure(`/`heap.native(`
in the evaluator either precedes any eval, reads a freshly-unpacked id, or (line 844) is a
documented no-GC symbol lookup; `apply_closure` was already correct. `record_tw_entry` was
the lone instance. The `vm_tail_arm_compaction` test loses the temporary VM pin (added
while gating the differential job) and runs on both engines again.

## 2026-07-28 (later) — map-order / truthiness / set doc fixes; string-seqability prototyped then rejected

A language-ergonomics review (syntax deep-dive) surfaced a batch of doc-drift and one
tempting-but-wrong feature.

**String seqability — prototyped, then reverted (kept as institutional memory).** We
briefly made a string seqable as its code points (native `first`/`rest` + prelude
`seq`/`nth` + widened checker `seq`/`seqable` domains), so `(map upper "abc")` ⇒
`("A" "B" "C")`. It built clean and passed the suite, but was **reverted before
commit** on reflection: for a language whose north star is a *text editor*, making the
ergonomic default (`map`/`first`/`rest` over a string) operate on **code points**
nudges toward the exact cluster-corruption the grapheme API exists to prevent —
`(first "e\u{301}")` returns a broken half-cluster. It also silently resolves the
codepoint-vs-grapheme choice the 2026-07-26 deferral deliberately kept *explicit*, and
the motivating friction was overstated (the JSON/regex parsers already index an
`int`-codepoint vector via `string->codepoints` for speed, so string-seqability
wouldn't touch them). **Decision: strings stay opaque; choose your unit explicitly —
`string->list` (code points) or `string->graphemes` (clusters).** The ROADMAP item
stays open. (Revisit only if a grapheme-default seq is wanted, which splits
`count`/seq-length and diverges from `char-at`.)

**Doc-drift fixes the review caught** (no behaviour change):
- **Map iteration order.** `language.md`/`spec.md`/`brood-for-claude.md` claimed maps
  were "insertion-ordered" — false since the ADR-030→ADR-040 CHAMP migration. Order is
  hash-derived (canonical per key-set, so two `=` maps iterate alike, but neither
  insertion order nor sorted, and not stable across builds). Reworded everywhere to
  "unspecified — sort the keys if order matters." (ADR-040's own "no ADR-030 contract
  broken" claim was the original slip; left as historical record.)
- **Empty-list falsy asymmetry.** Documented loudly that while `[]`/`{}`/`#{}`/`""`
  are truthy, the empty **list** is falsy because `()` ≡ `nil` — so `(empty? x)`, not
  a bare `(if a-maybe-list …)`. Added to `language.md` Truthiness, `brood-for-claude`,
  and the `writing-brood` skill.
- **`brood-for-claude` staleness.** Its set entry still said "no `#{}` literal or
  `set?` yet — test with `map?`" and "a set is a map of element→true" (both obsolete
  since ADR-060 first-class sets); the `writing-brood` skill said the same. Both
  rewritten. `set?` added to the predicate list.

MCP/LSP need no separate change: they read the same builtin/prelude docstrings, and
these were doc-comment fixes in the reference `.md` files, not builtin signatures.

## 2026-07-28 (later still) — the display protocol: records customize screen printing (ADR-171)

The "upgrade Brood to use `defability`" audit came back with a short, deliberate list:
only **two** genuine candidates (everything else in `std/` is a correctly-closed
`cond`/state-machine per ADR-011). The headline one — value rendering wanting
third-party extension, i.e. Elixir's `String.Chars` — is now shipped.

**`std/show.blsp`** (`require 'show`): a `Display` ability with one op `to-str`
(value → display string), `:default` → the native `str`. Loading it installs a hook
into a new prelude dynamic var **`*show*`** (nil by default), and the screen printers
`print`/`println`/`eprint`/`eprintln` route each argument through it. The hook touches
only **records**; every built-in passes through to the fast native renderer, so there
is zero dispatch cost for the common case and — with `show` unloaded — no behavior
change whatsoever (one `(if *show* …)` branch). A record then customizes its screen
form: `(impl Display money/usd (to-str [m] (str "$" …)))` ⇒ `(println (usd 1050))`
prints `$10.50` instead of `{:__id__ :money/usd, :cents 1050}`. `(binding (*show* nil)
…)` disables it; `(to-str x)` is the explicit protocol call for use inside `str`/`fmt`.

Scope is the screen printers only (the request was "printing to screen"): `str`/`pr-str`
/`fmt` stay native — they are the hottest paths and a niche benefit doesn't justify
routing every error message through a Brood ability. Default record printing keeps its
`:__id__` (intended Elixir-struct semantics, ADR-130); the protocol is the *override*
seam, not a reason to change the default. Full rationale in **ADR-171**.

Tests: `tests/show_test.blsp` (12, incl. cross-process — a record `send`s home and prints
via the protocol in another process, since `*show*` is a global and records deep-copy).
`std/show.blsp` + the test check clean; the full non-conformance suite is green (the
prelude print-path change is a no-op until `show` is loaded).

**Two findings along the way.** (1) `print`/`println` **space-join** their args
(`%render`, Python-style), distinct from `str`'s concatenation — worth remembering.
(2) First cross-module use of a `defability` op surfaced a **checker gap**: a `:use`d
ability op from a *loose disk* module (not embedded, not in a project) is flagged
`unbound symbol` even though it runs — embedded modules (`show`) and same-module use
resolve fine. Filed as a follow-up; it does not affect the shipped module. The second
audit candidate (`json--emit` → a `JsonEncode` ability, so user records serialize
instead of hitting the `else (error …)` tail) is the same shape and is left as a
documented follow-up.

**Follow-up (same day) — `Inspect` + a locale-aware prototype.** Added the companion
**`Inspect`** ability (`(inspect x)` debug form, `:default` → `pr-str`, + `inspectln`);
deliberately not wired into `pr-str` (round-trip guarantee). Worked the money / i18n
question into `tests/show_localize_test.blsp`: a `Localize` ability `(localize [self
locale])` (dispatch on value type, third-party-extensible) composed with `Display`
reading an ambient `*locale*` dynamic var, so plain `(println money)` localizes and
`(binding (*locale* :de) …)` switches it — per-process, verified across a spawn. And a
cross-module demo confirmed the **impl-shipping** model: a library that puts `impl
Display` at its module top level (Elixir's `defimpl`) makes a consumer's `(:use bank)` +
`(println money)` show `$10.50` with no mention of `show`. The open design question —
splitting the ability from the activation so a library can register an impl without
flipping global print — is recorded in ADR-171, deferred (ADR-011).

## 2026-07-28 (design) — Abilities v2 decided: app-sovereign coherence, `impl`/`bridge` (ADR-172)

A design review turned the opt-in `Display` protocol (ADR-171) into a full rethink of the
ability system's authority model, recorded as **ADR-172** (design accepted, not yet built).
The reframe: the axis that matters isn't coherence, it's **authority** — the app must
outrank every library — and the goal is compile-time guarantees *without* losing hot
reload. The decided model:

- **`impl` what you own** (the ability or the record type — Rust's orphan rule); built-ins
  are owned by nobody, so only an ability's owner may impl for them.
- **`bridge` what you link** — an app-only, greppable form for deliberate cross-library
  glue (the sanctioned orphan site). A glue *package* is a module of `bridge` forms,
  inert until the app authorizes it via the manifest's `:bridges`. This keeps Elixir's
  "impl a foreign type" capability minus its silent/transitive footgun.
- **App sovereignty** — the app may impl/bridge anything and wins; precedence
  `app > type-owner > ability-owner > :default > native`.
- **Compile-enforced, live-safe** — coherence/exhaustiveness/bridge rules are a hard
  reject at `nest check`/CI, re-run on reload, advisory in the live image.
- **Dispatch specialized via the IC/JIT with deopt-on-reload**; `:sealed` abilities go
  fully static; the runtime `*impls*` registry becomes the backstop, not a freeze.
- **Display becomes always-on core** (records only, app-gated, guarded), superseding
  ADR-171's opt-in `show`.

The through-line — *libraries propose, the app disposes* — unifies `:bridges` and
`display-on` as the same act (the app authorizing a borrowed/ambient effect). Needs
ADR-070 (package-rooted namespaces) for the clean app/library line; interim uses the
program's root namespace. Ships-today `std/show.blsp` stays the interim runtime
implementation until v2 is built.

## 2026-07-28 (impl) — Abilities v2 slice 1: optional + dev dependencies in the manifest

Started building ADR-172, from the bottom of the staged plan — the one slice with no
blockers (it doesn't need ADR-070 or any kernel work). The package manifest now expresses
the two dependency distinctions the bridge story needs:

- **`:optional true`** on any dep entry (`[foo :git … :optional true]`) — declared but not
  force-installed, the seam a bridge/glue package rides. `project-parse-dep` normalises it
  onto every dep map (default `false`); full resolution semantics (peer presence) land with
  the bridge slice.
- **`:dev-dependencies`** — a second manifest list parsed exactly like `:dependencies`, each
  map tagged `:dev true` and kept in its own `*project-dev-dependencies*` slot (never mixed
  into `*project-dependencies*`), so a release bundle / a published package's declared deps
  can drop test-only deps. Resolver + bundle consumption is the next step.

All in `std/tool/project.blsp` (parsing/normalisation) + the manifest macro docstring.
Tests: a new `:optional`/`:dev-dependencies` block in `project_test.blsp`; the existing
dep-map assertions in `project_test`/`package_test` updated for the new `:optional false`
key. Full non-conformance suite green, `nest check` clean, formatted.
## 2026-07-28 — the GC's forwarding tables were hash maps; `sort` −17.6%

Chasing the benchmark standing (`sort`/`bintree`/`nbody` are the three 6/7 rows within 8% of
a rank) led to the collector. `sort` first, because it is also 22% of the aggregate.

**Measured, not assumed.** Phase-isolating the row in separate processes: building the 375k
list ≈65 ms, walking it ≈21 ms, `sort` on a *list* ≈97 ms but on a *vector* ≈47 ms. That 50 ms
gap is not the sort — it is materializing 375k items. Raising `BROOD_GC_FLOOR` so collections
stop firing took the whole row 210 → 132 ms, which said GC, not allocation. `(gc-stats)` then
gave the number outright: **4 collections, 946,464 objects copied, 95.7 ms of pause in a 158 ms
run — 61% of the row, at 101 ns per copied object.** Three hundred cycles to move 48 bytes.

**The cause: `FlushForward` kept its forwarding pointers in ten `HashMap<u32, u32>`s.** Every
copied object paid a SipHash probe plus an insert, and the insert rehashed as the table grew
into the hundreds of thousands. But the keys *are slab indices* — dense, and bounded by the
source slab's length — so they never needed hashing at all. Replaced with a dense `Vec<u32>`
(`FwdTable`, `u32::MAX` = not-yet-copied), sized from the source generation up front.

Same run afterwards: **same 4 collections, same 946,464 objects copied, pause 95.7 → 44.6 ms**
(101 → 47 ns/object), wall 158 → 113 ms. Identical work, half the cost — which is the shape a
real fix should have.

Suite A/B against `4f5117e4`, best-of-7: **`sort` −17.6%** (216 → 178 ms), `json` −4.1%,
`persistent-map` −3.7%, `pipeline` −2.1%, `wordcount` −1.9%; `fib` flat.

**Why `bintree` and `nbody` did not move, which is the useful part.** They are not GC-*copy*
bound — `(gc-stats)` per row: `bintree` 11 collections but only 45,175 objects copied (5.0 ms,
~4% of the row), `nbody` 8 collections copying **798** objects (6.2 ms), `wordcount` zero
collections. Their objects die young, so there is nothing to forward. The nursery-size dial
splits them the same way: `BROOD_GC_FLOOR=2000000` takes `sort` −37% and `persistent-map` −12%
but makes `bintree` **+19% worse** and `nbody` +9% worse — a bigger nursery helps live-set
builders and hurts churners, so it is not a knob to turn globally.

**Open, for the next pass:** `nbody` spends ~770 µs *per collection* while copying ~100
objects, i.e. a fixed per-collection cost unrelated to live data. Two suspects, both in
`minor_collect`: it rebuilds the whole `form_pos` map on every collection, and
`Slabs::with_capacity_like` allocates a fresh nursery sized to the outgoing one each flip.
Unmeasured — do not act on it before profiling, since this session has already had two
confidently-named suspects turn out wrong.

## 2026-07-28 (later) — `def`'d data was a 70× cliff: the JIT deopted on every shared-region read

Continuing the standing work. After the forwarding-table fix, `sort` was still only 1.4×
faster with the JIT than without it (`bintree` 3.3×, `matmul` 7×) — so something in that row
was refusing to run native.

**The measurement that found it.** Walking a pair in a JIT'd loop, 2 M iterations, with the
pair created **locally** vs the identical pair `def`'d:

| | LOCAL | RUNTIME (`def`'d) |
|---|---|---|
| `first` | 7 ms (~1 ns) | **161 ms (~77 ns)** |
| `vector-ref` | 8 ms | **210 ms (~101 ns)** |

Then the confirming run: with `BROOD_NO_JIT=1` those same loops cost 137 ms and 210 ms — i.e.
**the RUNTIME figures were already the interpreter's**. The JIT was contributing nothing.

**Why.** `emit_prim1`'s inline `first`/`rest` checks the handle's region and **deopts** on
anything but LOCAL. The deopt fires per element, the arm's consecutive-deopt counter bails it,
and the whole loop reverts to the VM. The old comment called non-LOCAL "uncommon on hot
cons-list paths" — but a global holding a data structure is ordinary Brood. `sort` does
`(def data (sort …))` and then walks it; `matmul` derefs a `def`'d matrix ~16 M times.

**What it is not.** The obvious fix — hoist a shared-region slab base and inline
`base + idx*48` like the LOCAL path — **cannot work**: RUNTIME slabs are `boxcar::Vec`, chunked
rather than contiguous precisely so they can be appended lock-free, so there is no single base
pointer. (I wrote the accessors before discovering this and reverted them.) The `Arc::clone` in
`code_gen_pinned` is also *not* the cost — removing it moved 161 → 148 ms, ~8%.

So the fix is simply to stop deopting: non-LOCAL regions now call the same `car`/`cdr`
callbacks the no-hoist path already used, joined with the inline result through a block
parameter. One call per read instead of surrendering the loop. **RUNTIME `first`: 144 → 36 ms
(4×)**, LOCAL unchanged at 7 ms.

Suite A/B vs `46db4405`, best-of-7: **`sort` −17.3%** (179 → 148 ms); everything else flat,
which is the expected shape — this fires only on `def`'d heap data. With the forwarding fix,
`sort` is **216 → 148 ms, −31.5%** across the two changes.

`vector_ref` (dynamic index) already fell back to the FFI rather than deopting — its own doc
cites "matmul's def'd rows". Its constant-index sibling `inline_vec_ref` still deopts, and is
the obvious next candidate; I left it alone because measurement said the rows it would help are
not bailing (`nbody` 352 ms JIT vs 1112 ms VM, `matmul` 159 vs 1126), so the payoff is
unproven. Worth doing on evidence, not on symmetry.

## 2026-07-28 (evening) — two JIT deopt cliffs, both real, both NOT worth shipping

Hunting more of the morning's shared-region cliff with `BROOD_DEOPT_TRACE=1` across the weak
rows. One arm showed up: **`nbody`'s `advance-body` deopts 16 times and is then permanently
BAILED** to the interpreter — its core physics function. `bintree`, `wordcount`, `pipeline` and
`nqueens` are clean.

Bisecting a minimal repro found **two** independent per-call deopts, both genuine:

1. **`inline_vec_ref` deopts on non-LOCAL.** The constant-index `(nth bi 0)` on a `def`'d
   vector-of-vectors — the dynamic-index sibling `vector_ref` already falls back to the FFI
   instead (its doc even cites "matmul's def'd rows"). The fallback block already existed in
   `inline_vec_ref`, unreachable, named `dead_ffi`.
2. **Arithmetic with two type-erased operands guesses `int`.** `emit_prim2` takes the float
   path only if an operand is *statically* float (or the arm has a float slot); with two
   `Op::Handle`s — two vector reads — it falls through to `as_int`, which deopts on a `Float`.
   Per evaluation, with no checkpoint, so the arm re-runs from the top on the VM. In a minimal
   repro: **~497k deopts over 500k iterations**.

Both were fixed — (2) by dispatching on the runtime tags into a float or int block joined
through a boxed `Handle`. The repro went **495,751 deopts → 0**, and all six benchmark
checksums stayed identical to `BROOD_NO_JIT=1`.

**And the suite did not care.** A/B vs `c9d3fac8`, best-of-7 with the movers re-run solo at
best-of-11: `nbody` **+2.2% worse**, `nqueens` −1.0%, everything else flat. With only fix (1):
`nbody` +1.4% worse, `sort` +0.7%, the rest flat. So both changes were **reverted** — they cost
a few instructions on paths where the old guess was right, and buy nothing where it was wrong,
because the arms that hit the bad case were *already* bailed to the VM, where a deopt is free.

Worth recording rather than re-discovering: **a deopt cliff is only worth closing if the arm
would otherwise be running native.** `nbody`'s `advance-body` still deopts 16 times after both
fixes, so its bail has a third cause I did not find; whatever that is, it is upstream of these
two, and closing them without it changes nothing. The evidence trail (deopt counts per
construct, the `Prim2SlotSlot SetLocal Prim2SlotInt Prim2SlotInt Prim2` fingerprint) is in this
entry so the next attempt starts from the third cause, not the first two.

## 2026-07-28 (evening) — 300K processes vs the BEAM: spawn rate is fine, memory and *scaling* are not

Measured head-to-head at the scale that matters for a process-per-connection design,
300,000 processes spawned and left **alive** (parked in `receive`):

| | Brood | Elixir | |
|---|---|---|---|
| spawn time | 575 ms | 438 ms | 1.3× slower |
| resident | 1.58 GB | 907 MB | 1.74× heavier |
| **per process** | **5.4 KB** | **2.68 KB** | |

So the spawn *rate* is genuinely competitive — 675 ns/process after the registry sharding
(`1e64db5e`). The memory gap is the real one, and it points at the per-process `Heap`: two
`Slabs` of eleven `Vec`s each, plus caches and maps, against the BEAM's ~330-word process.
That is the case for pooling/recycling process heaps rather than constructing them.

**A parallel-scaling scare, which turned out to be my own measurement error — recorded
because the error is the instructive part.** `pfib` and a pure-integer burn both appeared to
scale only 2.3× on a 6-core/12-thread i5-11500H, with *two* workers showing **no speedup at
all** over one. That reads as a serious serialization bug.

It was not. **`BROOD_J=1` does not give one worker** — `worker_count()` floors the real pool
at 2 (the documented spare that lets a dirty-blocked worker be drained). So the "1 worker" and
"2 worker" runs were *the same configuration*, which manufactured both the flat 1→2 result and
a baseline that halved every speedup computed from it.

Measured properly, from a true 2-worker baseline, going 2 → 12 workers on identical hardware
and an identical workload:

| | 2 → 12 workers |
|---|---|
| machine ceiling (12 independent OS processes) | **3.0×** |
| **Brood** | **2.5×** (83% of ceiling) |
| Elixir / BEAM (`+S 2` → `+S 12`) | 2.4× (80%) |

So Brood's parallel scaling is **on par with the BEAM and near the hardware ceiling**. There
is no scheduler serialization to hunt. (The distribution machinery was never in doubt either:
`(sched-stats)` showed 99 steals for 100 processes at 2 workers, 100 at 12, peak-threads
matching the pool.)

Two lessons worth keeping. First, **a "1×" speedup should have been suspicious enough to check
the knob before the code** — no real contention produces *exactly* zero gain. Second, a
control matters: running N independent OS processes established the machine's own 3.0× ceiling
and is what made 2.5× legible as "83% of available" rather than "poor".

**Coverage gap found while checking:** `work_stealing.rs`, `live_migration.rs`,
`preemption.rs` and `concurrency_race.rs` prove the mechanisms are live and correct, but
**nothing asserts balance or scaling**, and `(sched-stats)` is aggregate-only — there is no
per-worker breakdown to write such a test against. A scaling assertion (embarrassingly
parallel work must beat 1 worker by ≥ N×) would have caught the 1.0× at J=2 immediately.

## 2026-07-28 (impl) — Abilities v2 slice 1b: dev-deps resolve for dev, excluded from release

Made the manifest flags from slice 1 actually do something end to end. Two seams in
`std/tool/project.blsp`:

- **`project--ensure-deps-on-path`** now load-paths `(append *project-dependencies*
  *project-dev-dependencies*)`, so a `nest test`/`nest run` in a dev image resolves and
  fetches dev-deps too — exactly like Cargo's dev-dependencies being available for tests
  locally.
- **`bundle-collect`** reads `*project-dependencies*` alone (unchanged), so a
  `:dev-dependencies` is deliberately **excluded from the shipped bundle** — the payoff of
  keeping dev-deps in their own slot.

Verified end to end with a scratch project: a `:dev-dependencies [[devtool :path "../dt"]]`
is resolvable from a test (`(:use devtool)` passes under `nest test`), and
`(bundle-collect root)` omits it (`devtool in bundle? false`). Optional-dep *resolution*
(peer presence) still lands with the bridge slice; this is the dev-dep half. Suite green,
`nest check` clean, formatted.

## 2026-07-28 (design) — dropped `:bridges` from ADR-172: reusable glue is a package of functions

A "do we really need `:bridges`?" question exposed a contradiction in the design: `bridge`
was declared **app-only**, yet a glue *package* (a module of `bridge` forms authorized by a
manifest `:bridges` list) is not the app. `:bridges` existed only to reconcile that — so it
was dropped. `bridge` is now strictly app-only, full stop. Reusable glue doesn't need orphan
impls in a package: the package exports plain **conversion functions**, and the **app** writes
the `bridge` calling them (`(bridge JsonEncode (ecto/decimal (encode [d] (json-ecto/decimal->json
d))))`). The app always declares the link — you can never get a bridge you didn't write — and
`bridge` staying unambiguously app-only shrinks the ADR-070 dependency ("who is the app" only
has to mean "the root/entry program"). The turnkey "add package X and it just works" is the one
thing lost, but that *is* the silent/transitive behaviour the model rejects. Knock-on: `:optional`
(shipped, slice 1) loses its strongest motivation (gating a glue package on both libs present),
falling back to the generic Cargo/Elixir optional dependency; kept for now, worth revisiting.
ADR-172 §2/§4/§5/§9/§10 + the walkthrough artifact updated.

## 2026-07-28 (impl) — Abilities v2 slice 4 (part): deterministic precedence by tier

`std/ability.blsp` now resolves competing impls by **precedence tier, not load order**
(ADR-172 §3). `defability` records its owning namespace (`*ability-owner*`); each
`register-impl` computes a tier from provenance via `impl-rank` — **type-owner (3) >
ability-owner (2) > other (1)** — and the registry keeps the highest-tier impl for each
`(ability, op, id)` slot: a strictly lower tier is dropped, so the winner is the same
regardless of registration order. Done as a guard *at registration*, so `impl-for`
(dispatch) stays a plain map-get — no per-call tier walk. Additive: single-impl cases and
same-tier hot-reload are unchanged, so the whole ability/show suite stays green (+4
precedence tests in `ability_test.blsp`, now 28).

**Deferred (needs ADR-070):** the top **app** tier — the program overriding *anything*.
Distinguishing an app module from a library one needs package-rooted namespaces; until
then an app override registers as `other` and wins only by same-tier last-write (the
pre-ADR-172 behaviour). The type-owner-beats-ability-owner half — fully determinable now —
is what this slice makes deterministic.

## 2026-07-28 (impl) — `with`: Elixir-style sequential match-binding (prelude macro)

Added `with` to `std/prelude.blsp` next to `if-let`/`when-let`. Flat `pattern expr`
pairs (the `let` shape); each `expr` is matched against its `pattern` in order, the
first value that fails its pattern short-circuits, and the body runs only when every
step matched with all bindings in scope. Pure sugar over nested `match` — **no new
special form** (keeps the core small; a step lowers to `(match expr (pattern <rest>)
(miss <miss-arm>))`). A trailing `:else` section is a set of `match` clauses run
against the short-circuited value (like Elixir's `else`); with no `:else`, that value
falls straight through — so an `[:error …]` becomes the result untouched.

Considered and rejected a Result/Either **monad** abstraction: Brood is dynamic, so a
"monad" here is just combinators with grander names and no law-enforcement — and it
fights the small-core / ADR-011 (defer power) rules. `with` is exactly the pragmatic
answer Elixir itself chose over monads. Tests in `tests/ergonomics_test.blsp` (happy
path, fall-through, `:else`, empty bindings, map/tuple patterns) + cross-process
coverage (a worker builds+sends `[:ok …]` result tuples, parent matches with `with`,
plus a fan-out/fan-in fold). Docs in `docs/language.md` § binding-conditionals.

## 2026-07-28 (impl) — `spy`: homoiconic tree-tracing debug macro (ADR-173)

Borrowed Elixir's `dbg` but did it more Lisp. `(spy expr)` evaluates `expr`, traces
every evaluated subexpression's value in evaluation order, and returns the value
unchanged (referentially transparent — wrap/drop it freely). Named `spy` (Lisp
tradition) over `dbg`. Prelude macro in `std/prelude.blsp` (not a Rust builtin).

Design (ADR-173): rather than `dbg`'s fixed special-case set + AST-to-source
reconstruction, `spy` exploits homoiconicity — fully macroexpands the form
(re-expanding at every node, since `macroexpand` only resolves the outer head) and
instruments each evaluated position **in place**. That preserves laziness for free (an
untaken `if` branch / short-circuited `and` tail never traces) and makes pipelines a
non-case: `(-> x f g)` → `(g (f x))`, each stage traced by the ordinary call rule.
`fn` bodies, `quote`, `quasiquote` left opaque. Descends into `if`/`do`/`let`/`letrec`
+ calls; other special forms trace their top value only (sound, conservative).

Second bet: a swappable **`*spy-sink*`** (`defdyn`) carrying structured entries
(`:enter`/`:node`/`:exit` maps) — so a host (editor, `nest observe`, a test) captures
the trace as *data*, not text; default pretty-prints an indented tree to stderr. This
is the seam for future editor inline-values (M2/M3), and it subsumes "no-op in
production" (rebind to a no-op) without an added gate.

Tests: `tests/spy_test.blsp` (13) — transparency, trace-as-data via a capturing sink,
laziness (untaken branch / short-circuit absent), `let` RHS+body, fn-body opacity,
quoted-data passthrough, error propagation, + cross-process (`:isolated`: a worker
spy-computes and sends a value, fan-out/fan-in fold). Docs in `language.md`; ADR-173.

## 2026-07-28 (fix) — `make install` installed a REPL-less `brood` (dev-tools split)

`make install` built `brood`/`nest` `--no-default-features` (the lean runtime that
gets embedded into `nest` for `nest release` app bundling) and copied *those* onto
`$PREFIX/bin`. But the lean build strips `dev-tools`, i.e. the `DEV_MODULES`
(`repl`/`test`/`observer`/`mcp`) — so the installed `brood` couldn't start its own
REPL (`require: cannot find module 'repl'`), and this had been true for every
`make install`. Root cause: one binary was serving two opposite needs — lean for
*embedding/shipping*, full for *interactive use on PATH*.

Fix (Makefile): split them. `release-brood` still builds the LEAN brood
(`RUN_FEATURES`) — the embed base + what `make ab` measures — unchanged. `release`
now builds `nest` and the installed `brood` with `INSTALL_FEATURES` (= `RUN_FEATURES`
+ `brood/dev-tools`); the dev `brood` is rebuilt *after* `nest` has already baked in
the lean copy, so it overwrites the on-disk file without changing what apps ship.
Net: `nest release` bundles stay lean, the `brood`/`nest` on your PATH get the REPL
and `nest test`/`observe`/`mcp` back. Costs one extra ~12s brood build at install
time. Verified: `make install` then `brood` REPLs; the embedded runtime is still
lean (nest embeds release-brood's output, built before the dev overwrite). Docs:
`docs/release.md` note added. (Surfaced while dogfooding `spy` in the REPL.)

## 2026-07-28 (late) — a parked process kept its garbage *and* its capacity; trim at park

Picking up the memory-per-process gap against the BEAM (5.4 KB vs 2.68 KB). The base figure
turned out to be the least interesting part.

**What a parked process actually holds.** 100k processes, each running a body then parking in
`receive`, measured by RSS delta:

| child body | KB/process |
|---|---|
| bare park (never allocated) | 5.40 |
| cons 100 pairs, drop them, park | 14.42 |
| cons 1,000 pairs, drop them, park | **54.70** |

The list is *dead* in every case — dropped before the `receive`. A process that consed 1,000
pairs (~48 KB) keeps essentially all of it, forever, while parked. For a process-per-connection
server that is the difference between fitting a million and not.

**Two mechanisms, separated by measurement.** Adding an explicit `(gc-collect)` before the park
took the 1,000-pair case 54.1 → 19.0 KB, and bare-park is 5.4 — so:

1. **Uncollected garbage** (54 → 19 KB): a parked process never reaches another safepoint, so
   nothing ever collects it.
2. **Retained capacity** (19 → 5.4 KB): after collecting, the slab `Vec`s still hold their
   high-water capacity, and a nursery flip *deliberately* preserves it
   (`Slabs::with_capacity_like`) so the next cycle skips the doubling ladder. Correct for a
   running process; wrong for one that may never run again.

**`Heap::trim_parked`** does both — collect, then `shrink_to_fit` the slabs, roots and env
roots — called from `park_on_receive` at the one moment we know the process has nothing to do.
This is Erlang's `hibernate/0`, applied automatically instead of by hand.

| child body | before | after |
|---|---|---|
| bare park | 5.40 KB | 5.40 KB |
| cons 100, park | 14.42 KB | **8.48 KB** |
| cons 1,000, park | 54.70 KB | **8.50 KB** |

**Getting the gate right took three tries, and the two failures are the instructive part.**
The gate has to keep a latency-sensitive responder — which parks constantly with a small heap —
off the collection path, without leaving accumulated garbage behind.

1. **Absolute threshold, 32 KiB.** Latency perfect, and **memory non-monotonic in
   allocation**: the 1,000-cons process crossed the gate and trimmed to 8.5 KB while the
   100-cons process stayed under it and kept **14.5 KB**. Allocating *more* used *less*. Not
   shippable — and I had written the table out without noticing until it was pointed out.
2. **Absolute threshold, 4 KiB.** Monotonic (5.4 → 8.5 → 8.5) and `pingpong` **+193%**
   (213 → 624 ms): that row parks 200k times and now paid a collection on nearly every one.
3. **Growth since the last trim, with hysteresis.** The gate asks "has anything *new*
   accumulated?", not "is the heap big?". Marking the *pre-trim high-water* rather than the
   shrunken size is what makes it stick: marking the shrunken size lets the process regrow the
   threshold immediately and oscillate (`pingpong` +8.5%); against the high-water, a steady
   working set trims once and never again.

Plus one measurement fix: the gate runs on *every* park, and summing eleven `capacity()`
fields per generation (~25 ns) cost `ring` **+4.6%** on its own — the trims it gated cost
nothing. A three-field element-count probe took that to +2.8% and `spawn` to 0.0%.

**Final cost: `ring` +2.8%, `pingpong` +1.9%, `spawn`/`pfib`/`sort`/`bintree` flat**, against
6.4× less memory for a process that did work before parking. At 300k processes that is ~2.5 GB
against ~16 GB — the difference between fitting and not.

**Soundness** rests on an invariant the code already documents rather than on my reading of it:
`Suspended` holds only *control* state — its frames reference the operand stack and frame slots
by index, and those live on the process's own `roots`/`env_roots`, which `collect` traces. So
collecting at the park point is no more dangerous than collecting one instruction earlier, at
the safepoint the process would have hit had it kept running. The process owns its heap there
(no worker is running it), so the collection cannot race.

## 2026-07-28 (late) — `nbody`'s third deopt cause: narrowed, not solved

Following the trail left earlier (two deopt cliffs found, fixed, measured, reverted — the arms
they would have helped were already bailed to the VM). `advance-body` still deopts 16 times and
is then permanently BAILED, so nbody's physics runs interpreted. New evidence, so the next
attempt starts here rather than at the beginning:

- The deopt is **resumable** — `ckpt_slot: 14`, `resume_ip=17`, `depth=1` — unlike the minimal
  repro I built earlier, which had no checkpoint. So the shapes are not the same and the repro
  was misleading.
- `resume_ip=17` lands on a **`Call`**, in the region
  `… Call SetLocal Local JumpIfFalse Local Call Const Prim2 Jump …`. From the source that is
  `(newvel b i 0 …)` followed by the `[nvx nvy nvz]` destructuring of its result.
- **Both arms lower.** `advance-body` is arm 61; `newvel` is arm 67 (`SelfCall` + `MakeVector`,
  no checkpoint). So this is not a missing compilation — it is a *call boundary* between a
  self-recursive, vector-returning callee and a caller that immediately destructures.
- No other arm deopts in the whole benchmark, and `newvel` itself never appears in the trace.

What is NOT yet known: whether the deopt originates in the caller's guard on the returned value
or is a deopt outcome propagated out of the fast link. Answering that wants the CLIF for arm 61
around that call, or a counter on the fast-link outcome path — instrumentation rather than
reading. Left there deliberately: the last two cliffs on this row cost a day and bought nothing,
so the next attempt should confirm the payoff (does the arm run native afterwards, and does the
row move?) before the fix is written.

## 2026-07-28 (impl) — Abilities v2 slice 4 (rest): the app tier lands via package identity

The half slice 4 deferred — the top **app** tier, a program overriding *anything* — now
works, without waiting for full package-rooted namespaces (ADR-070). The insight: the app
tier only needs to answer *"does this namespace belong to the app or to a library?"*, which
is a name→owner lookup, not a rename of every namespace. So instead of prefixing namespaces,
we record **package identity**: a `defdyn *ns-package*` maps each namespace to its owning
package's name, populated by a static scan at project setup (reusing
`package--provided-modules` — no change to the hot `require`/`defmodule` path). A namespace is
"the app" when its package equals `*project-name*` (the project's own `:name`), or when it has
no recorded owner at all (root / REPL). `impl-rank` gains the top tier: **app (4) > type-owner
(3) > ability-owner (2) > other (1)**, deterministic by tier regardless of load order. End to
end: an app `impl` for `:int` beats a library's `impl` for the same slot, whichever registers
first.

Package identity also enriches diagnostics: `ns-package` resolves a namespace or a qualified
name to its package, and `trace-with-packages` tags each stack frame with its owning package —
so a trace reads "which library was this frame in", not just "which module".

**Display protocol activation moved to an explicit app step** (ADR-172 §5/§8): loading `show`
now only makes the protocol *available* (the ability + a library's `impl Display` proposals);
the screen printers honor it after the app calls `(display-on)` (installs the `*show*` hook),
undone by `(display-off)` or `(binding (*show* nil) …)`. A library ships proposals; the app
disposes. This is interim scaffolding — it vanishes when `Display` becomes always-on core
(slice 6) — and is kept deliberately un-polished (the safety it approximates is owner-only
coherence, not an activation gate).

**Bug hunt — three defects found and fixed** (all with reverting-fix-fails-the-test regression
coverage): (1) HIGH — an app/root impl (`from` = nil) registered *before* a library impl was
silently clobbered, because `(if prev-from … 0)` read a nil-from incumbent as rank 0; keyed off
`(contains? *impl-from* …)` (presence, not truthiness) so a nil-from incumbent keeps its real
tier. (2) `ns-package` crashed on a trace frame with no `:fn` via `(symbol nil)` — `when`-guarded.
(3) a dependency whose `:name` equals the project's package name would wrongly get the app tier —
rejected at resolve time with a rename message. Suites: ability 32, project 75, package 65, show
12 + 5 — all green, incl. under `BROOD_GC_STRESS=1` and the tree-walker.

Slices 2, 3, 5, 6 (`bridge`, coherence checking, dispatch specialization, always-on `Display`)
remain.

## 2026-07-28 (design) — ADR-172 amended: `bridge` and the orphan rule dropped before build

Pulled §1/§2 of ADR-172 apart and found the pair unnecessary. **`bridge` has no runtime
substance** — it expands to the identical `register-impl` call as `impl`, same `(current-ns)`
tag, same app tier; its sole purpose was to be the sanctioned, greppable, app-only channel for
*orphan* impls so that `impl` could be restricted to owned slots. But the **orphan rule itself is
premature**: "a library may not `impl` a type/ability it doesn't own" guards a
multi-third-party-library collision that greenfield Brood (one app + `std`) does not have — and
adopting it would break two capabilities we want and already have: impl'ing an ability for a
**primitive** id (`:int`) from anywhere, and impl'ing a **library** ability (`Display`) for your
own records. App sovereignty is already delivered by the **precedence ladder** (slice 4, shipped):
`app > type-owner > ability-owner > other`, same-tier collisions warned. And the app/library line
is **computable** (package identity), so if a real orphan conflict ever appears, orphan
authorization is a **lint on plain `impl`** (advisory-live / hard-CI), never a second form.

So: abilities stay the open ADR-168 registry, made deterministic by the ladder; `impl` is legal
for any ability and any id; `bridge` is not built. Same reasoning that dropped `:bridges` (a second
mechanism with no substance, for a restriction we don't adopt), applied one level up. Verified the
three cases still run: `impl` for `:int`/`:string`, `Display` on a user record (`$500`), and an
orphan `Display :int` (`#42`). ADR-172 status + §-map amended; ROADMAP marks slices 2+3 ❌ dropped,
5 (dispatch specialization) + 6 (`Display` to core) the substantive slices that remain;
`language.md` "planned direction" block and the `:optional` comment de-bridged.

## 2026-07-28 (impl) — Abilities v2 slice 6: abilities + Display are CORE (folded into the prelude)

`Display` was never more than an ability plus one line wiring `*show*` — so making it "always
on" meant making the *ability system* core. Folded `std/ability.blsp` + `std/show.blsp` into
`std/prelude.blsp` (both files deleted): `defability`/`impl`/`defrecord*`, the registries +
dispatch (`identity-of`/`impl-for`/`register-*`/precedence), and the `Display`/`Inspect`
abilities with their `:default` impls now live at the root, and the prelude tail sets
`(def *show* show--print-hook)`. Result: a record customizes how it prints with just
`(impl Display …)` — **no `(require 'show)`, no `(:use ability)`, no `display-on`** — and the
protocol is frozen once into the shared prelude region (zero per-runtime cost).

The path there ruled out two alternatives: `(require 'show)` at the prelude tail **crashed boot**
(module macros like `defability` aren't live during the frozen prelude build, so `show`'s
`(:use ability)` can't expand); a one-line `Interp::new` post-boot load worked but was a Rust
hook doing Brood policy and reloaded per runtime. Folding into the prelude wins because the boot
loop already propagates macros form-by-form — `defability` defined early is visible to `Display`
later in the same pass.

Fallout, all fixed: the checker's ability pass (`types/check/protocol.rs`) matched the old
*qualified* emit names (`ability/register-impl`, `ability/impl-for`, `ability/register-ability`,
`ability/register-sealed`) — now unqualified/root, so the four string matches were updated (the
missing-impl / sealed / conformance lints went silent until then). The `deftype` polymorphism
hint and a `basic.rs` require-semantics test (both asserted `(:use ability)`) were rewritten to
say "core, no import"; `check/tests.rs` dropped its `(require 'ability)` prefixes; the two
embedded-module entries were removed; the `show`/`show_localize` tests dropped `(display-on)`;
and the language/for-claude docs + the stdlib-module table were de-`:use`-d. `display-on`/
`display-off` are gone (`(binding (*show* nil) …)` still scopes it off). Verified: ability 32,
show 12, show_localize 5, json 30, checker ability 12, `basic` 102 — all green; `nest check`
exits 0. This completes ADR-172 §8; slice 5 (dispatch specialization) is the last open slice.

**Records unified — one `defrecord`, always identity-carrying.** With abilities core, the
`defrecord`/`defrecord*` split stopped earning its keep: the two forms had *diverged* (plain
`defrecord` gave per-field accessors but no identity/dispatch; `defrecord*` gave identity but no
accessors), forcing a silly either/or. Collapsed to **one `defrecord`** = positional constructor
+ per-field accessors + nominal `:__id__` identity + the record-shaped constructor `sig` (so a
value's identity flows through the checker to dispatch sites). `defrecord*` and the star are gone.
The only semantic change: a record is now **nominal, not structural** — never `=` to a bare map
with the same fields (Elixir-struct semantics), and `keys`/`count` include `:__id__` (use
`fields`/`record?`/`record-id` for the clean view). Blast radius was tiny — nothing depended on
record-`=`-bare-map (grepped); the 9 call sites were mechanical renames; `record_test.blsp` was
rewritten to the nominal semantics. Verified: record 12, ability 32, show 12, show_localize 5,
json 30, checker-ability 12, `basic` 102 — all green. This also answers the "why the star?"
question — there's no variant left to disambiguate.

## 2026-07-28 (impl) — process-native tracing debugger (std/tool/debug, ADR-174)

Built the actor-model answer to Elixir's `dbg` on the `spy` sink (ADR-173). The debugger
is a *process*, which dissolves `dbg`/`pry`'s two limits: `break` parks a process with NO
timeout (send snapshot → block on receive), and many processes hitting the same break each
park independently and fan into an inspectable queue — no single-session bottleneck.

Causal spans propagate TRANSPARENTLY across `spawn`: the debugger endpoint + current span
live in one dynamic, `*trace-context*`, and the kernel seeds a child's dynamics from it at
spawn (lifecycle.rs, reusing promote + push_dynamic → GC-safe; `#[cfg(dev-tools)]`-gated so
a lean release compiles it out entirely). So a plain `(spawn (fn () (break …)))` inherits
the debugger and parks with no re-wiring, and the debugger rebuilds a cross-process causal
tree. Traces are data: value-distribution / outliers debug the *population*; debug-report /
debug-watch (live) / debug-attach (interactive, key-driven resume) render it. New Heap
method `current_dynamic`. tests/debug_test.blsp: 10 cross-process tests; verified under
BROOD_GC_STRESS across concurrency/proc/scheduler/proctree/jit-shared-spawn.

Deferred (ADR-174, ADR-011): send-level causality — following a value through a *message*.
Perf-safe (cfg-gated Envelope, matcher untouched) but needs a GC-traced Value slot on the
Heap across ~8 collector sites, so it lands as its own GC-stress-gated pass, not bundled.

## 2026-07-28 (impl) — debugger send-level causality: context follows a message (ADR-174 §4)

Finished the deferred send-level slice: causality now follows a value A→B through a
*message*, so a long-lived server (never wired to the debugger) handles each request in
the *sender's* context — `dbg` can't do this at all. All `#[cfg(dev-tools)]`, so a lean
release is byte-identical (verified: `cargo check --no-default-features` clean).

Kernel: the durable context moved from a dynamic to a settable per-process `trace_context`
slot on the `Heap`, GC-traced where `dynamics` is (5 collector sites) + a `%trace-context`
/ `%set-trace-context` primitive pair. The mailbox message became an `Envelope { msg,
#[cfg] trace }` — uniform `.msg`, so `receive_match` is untouched and release is a zero-cost
newtype; `send` attaches the sender's context for a local pid, `receive` adopts it on pop.
A context is tagged **own** (set by `with-debugger`/`span`, propagated by `spawn`) vs.
**adopted** (from a message, not propagated onward) — a distinction a test forced: without
it the framework's own result messages leaked context into later test processes and hung an
unrelated "break with no debugger" case. Verified: debug suite 12 (incl. send-level +
leak-prevention), `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` clean, concurrency/gen/proc green.

## 2026-07-28 — REPL completion: list the candidates when Tab can't extend

Tab completion has been in the line editor since ADR-052, but only in its
insert-or-common-prefix form — which is invisible on an ambiguous prefix. Type `str`,
press Tab, and *nothing happens*: the common prefix is already typed, so there is
nothing to insert and nothing is drawn. From the outside that's indistinguishable from
"completion isn't implemented", which is exactly how it got reported.

Fixed by the readline convention: make progress if there is any, otherwise **show the
alternatives**. `lineedit--apply-completion` now attaches the candidate list as
`:completions` in precisely the case where its computed insertion equals the prefix it
was given (several candidates, shared prefix already spelled out), and the renderer
paints them in dim equal-width columns below the input — row-major, as bash lists.
Because the *insertion* decision and the *listing* decision are the same `cond`, they
can't disagree; a listing appears if and only if the keystroke did nothing visible.

**Statelessness was the design constraint.** A cycling menu or ghost text would both
need a mode — after Tab, what does the next key mean? — and a mode in a
`(state, key) -> state` editor means every command has to know about it. A listing needs
none: `lineedit-handle` drops `:completions` *before* dispatch, so it survives exactly
one keystroke and no command (including a user's own rebinding) can leave a stale one on
screen. Tab re-attaches it; anything else clears it.

**Geometry is the real constraint.** Rendering is relative — climb up over the previous
block, clear below, reprint, repark — so anything painted below the input has to be
climbed back over exactly. The single-line path now sums the hint row and the listing
rows into one `nbelow`, and the multi-line path's `up-to-cursor` spans the remaining form
lines *plus* the listing (`(- (+ (dec nlines) ncand) cur-row)`) — the off-by-N that a
naive append would have left, pinned by a test asserting the `[:up 3]`. Rows are capped
at `*lineedit-completion-max-rows*` (6) with a `… +N more` tail and truncated to
`width - 1`: a listing tall enough to scroll the terminal, or a row wide enough to wrap,
desyncs the cursor restore — the same limit the one-line signature hint already lives
under.

All of it is Brood (`std/editor/lineedit.blsp`), all pure but the two render calls, so
the layout is unit-tested with no terminal: 11 new cases covering the attach/no-attach
boundary, one-keystroke lifetime, column layout, narrow-terminal wrapping, over-long
truncation, the cap, and the multi-line repark. The two render calls were checked the
only way they can be — driving the real REPL through a pty and reading the escapes:
`str`+Tab gives 16 candidates in 3×6 with `[6A` back to col 11; `(map str`+Tab stacks the
signature hint *and* 6 listing rows and climbs `[7A`; a two-line `defn` climbs `[6A` to
row 1, col 14. `,help` gained a Tab line, since a feature nobody can see is worth
advertising.

Suite: 3669/3672, the 3 failures all `KI-14: deep JIT'd recursion` hard-killed at the
120s cap — a debug-build speed artifact (unrelated to this change; they exercise JSON
recursion depth, which never loads the editor).

## 2026-07-28 (impl) — Abilities v2 slice 5: the dispatch inline cache (`%dispatch`)

Ability dispatch was ~3.5× a direct call: each op did `(identity-of self)` then
`(impl-for [A op] id)` — two global-`*impls*` CHAMP lookups (hash the `[ability op]`
key, then the id) on top of the identity read. The Brood-side inline tricks were
marginal (~6–11%) and rightly rejected as hacks, so this does it properly: a **per-op
inline cache in the kernel**, mirroring the existing `GlobalIc`/`CallIc` machinery.

`%dispatch(impls, op-key, id)` (one Rust builtin, `Heap::vm_dispatch`) memoises the
resolved impl `fn` per op-key in a per-process map `ic[op-key] = (epoch, id, fn)`. A HIT
— same op-key, same id, current epoch — returns the cached fn with no `*impls*` touch; a
MISS resolves `impls[op-key][id]` (else `:default`) and caches. The op (from `defability`)
passes `*impls*` in, so the kernel stays decoupled from the global's name and the
resolution policy stays in Brood — the cache is a pure memo of `impl-for`, invisible to
the language (the user calls `(sz b)` and never sees `%dispatch`).

The elegant part is invalidation: the cache is validated by the shared `global_epoch`
(`runtime.version`), which is bumped by **every `def`** — so `register-impl`'s
`(def *impls*)` — **and by RUNTIME compaction**. So one epoch guard makes the cache
reload-safe (a redefined impl misses → re-resolves), GC-safe (a moved RUNTIME fn handle
misses; and the cached fn lives in the RUNTIME-promoted `*impls*`, stable under minor GC
+ rooted during the call), and cross-process-correct (a `def` in any process bumps the
shared version) — no new invalidation machinery, no write barrier, nothing user-visible.
`is_movable` gates caching as a belt-and-braces against ever storing a LOCAL handle.

Result: dispatch **~8.5s → ~5.5s** over 5M calls (best-of-3, release) — the overhead vs a
direct call (`~6.2s → ~3.2s`) roughly **halved**, ratio 3.5× → 2.4×. Verified correct
(ability 34 incl. two new reload/poly IC-deopt tests, record/show/json green), GC-safe
(debug per-deref tripwire + `GC_VERIFY` heap-verifier clean under `BROOD_GC_STRESS=1`),
reload-transparent (warm the cache, redefine the impl, next call is the new one), and
engine-agnostic (tree-walker `BROOD_VM=0` too). Remaining §7: compile-time static
resolution where the receiver type is known, and `:sealed` → a closed switch.

## 2026-07-28 — REPL: bounded printing, result history, rc file, auto-indent + M-q

Four things chosen off a "what would make the REPL awesome" list. Three landed; the
fourth turned into a kernel investigation that is *not* resolved — see the end.

**Bounded printing** (`std/prelude.blsp`). `pr-str` is faithful — whatever it returns
reads back — which is the right contract for serialization and the wrong one for a
screen: one 100k-element list at a prompt scrolls the session away and there was no way
to ask for less. So bounding is a separate function (`pr-str-bounded`) under two
`defdyn` knobs (`*print-length*` 100, `*print-level*` 8, `nil` = unlimited), not a flag
on `pr-str` — nothing that round-trips data can accidentally acquire an eliding printer,
and `pr-str` stays the escape hatch for seeing everything. Only containers are
traversed; every leaf and every value it doesn't know (a record with `impl Display`, a
rope, a table) goes to `pr-str` untouched, so custom printing keeps working. One marker,
`…`, for both bounds rather than Clojure's `…`/`#` pair: two symbols for one concept is
a thing to learn for no gain, and a bare `#` re-lexes as the start of a `#{…}` set to
anything reading the output back — which the REPL's own result highlighter does.

The depth bound tests on *entering* a container, not on a leaf. Getting that wrong is
what the first version did, and it shows up immediately: `*print-level* 2` on
`{:a {:b {:c 1}}}` elided each of the inner map's keys and values separately
(`{:a {:b {… …}}}`) instead of the subform (`{:a {:b …}}`).

**Result history + discovery + rc** (`std/tool/repl.blsp`). `*1 *2 *3` and `*e`; a value
you didn't think to name was previously just gone, and for anything expensive or
effectful "retype it" is the wrong answer. These work because `defdyn` is the only door
to an **ambient** (never-namespaced) name (ADR-151) — a plain `(def *1 …)` inside
`(defmodule repl …)` defines `repl/*1`, while the user typing `*1` means the bare root
name; `defdyn` also makes a later `def` from any module rebind that one root binding,
which is exactly what the loop needs. `def` and not `binding`: the value has to outlive
the form that produced it. An incomplete form deliberately does **not** land in `*e` —
it isn't a failure, and clobbering the error you're debugging with "read another line"
would be worse than nothing.

`,apropos` and `,search` finally expose `apropos`/`doc-search`, which had been sitting in
the prelude unreachable from the prompt, plus `,expand`/`,expand1` over `macroexpand`.
And a startup file (`$BROOD_RC` or `~/.broodrc.blsp`, then `./.broodrc.blsp`): every
customisation this REPL advertises — the keymap, the prompt, `*lineedit-candidates*` —
died with the session because there was nowhere to put it, so the rc is the payoff on
ADR-048's premise rather than a new feature. A broken rc reports and continues; a
customisation that locks you out of the tool you'd fix it with is not acceptable.

**Auto-indent + M-q** (`std/editor/lineedit.blsp`, `+ enclosing-open` in
`highlight.blsp`). Continuation lines started at column 0. The indent rule is *read off*
`std/format.blsp`'s actual output rather than invented, so typing and reformatting can't
disagree: `(` and `[` continue at the opener's column + 2, `{` at + 1. That's the whole
rule — no per-form table, because the header table changes what stays on line 1, never
the body indent.

`M-q` only accepts a reformat whose result reads back as the *same forms*. Not
belt-and-braces: `format-source` doesn't raise on incomplete input, it **completes** it,
so mid-typing `(defn f (x` came back as `(defn f (x ) )` — silently closing brackets the
user hadn't. Comparing parsed forms rejects that while still allowing the whitespace and
comment changes a formatter is entitled to make.

**Interruptible Ctrl-C: built, blocked, off by default.** Today Ctrl-C during a long eval
terminates the runtime (raw mode is only held for the *read*), so the price of stopping
one expression is the whole live image. The intended fix needs no evaluator change: run
each eval in a green process and `(exit pid :kill)` it. The kernel seam for that is two
primitives over `libc` (`%install-interrupt-handler` / `%interrupt-taken?`) — verified
working, and opt-in so a script keeps dying on Ctrl-C like any Unix program.

It doesn't work, because **a process spinning inside `eval`/`eval-string` never honours
the hard kill**:

    direct call                → DIED reason=:kill
    via eval-string            → SURVIVED
    via (eval (read-first …))  → SURVIVED

A REPL can't avoid `eval-string`, so the first Ctrl-C would issue a kill that does
nothing and the second would halt — two keypresses for today's one-keypress outcome.
That's a regression, so it's behind `BROOD_REPL_INTERRUPT=1` with the default untouched.

The root cause is **not** located, and the obvious guess was wrong. Adding a pending-kill
check at every plausible safepoint (`eval/mod.rs`'s `'tail:` loop top, both of
`exec_chunk`'s self-tail safepoints, `vm_run_bc`'s frame boundary) changed nothing, so
those edits were reverted rather than left speculative in a hot path. Instrumenting every
reduction path showed why: the child *does* accrue reductions (~1000× slower than the
direct call, i.e. tree-walked) yet reaches **no** rollover in `tick()`, `tick_capture()`,
or any patched site, with or without the JIT — only the parent pid rolls over, and its
kill flag is never set. So a nested eval's loop consumes its budget somewhere none of
those safepoints see; `charge_native`'s time-based accounting is the next suspect, since
`eval-string` is one long native call. Repro: `scratchpad/killtest2.blsp`.

Worth fixing on its own terms regardless of the REPL: any spawned process evaluating code
is currently unkillable, which reaches supervisors and `gen` servers too.
## 2026-07-28 — per-process compiled code is the per-process memory cost

Chasing the 4.58 GB that 300k live processes cost on the `spawn-live` benchmark (Elixir
holds the same 300k in 942 MB). Not a leak: per-process cost is flat and monotonic
(17.7 KB/proc at N=10k → 15.3 at N=300k, the decline just fixed overhead amortising).
Not the message either — payload length is irrelevant (k=0: 792 MB, k=64: 807 MB at N=50k).

The decomposition, N=100k, all processes kept alive:

| unit does | KB/proc |
|---|---|
| parks immediately, never runs | 6.26 |
| wakes, replies, parks again | 14.87 |
| same + one `(fold + 0 nil)` | 32.66 |

`(fold + 0 nil)` folds an **empty list** and costs ~18 KB. The cause is that
`vm_cache: RefCell<VmCacheMap<Option<Arc<CompiledClosure>>>>` and the inline-cache tables
(`vm_call_ics` / `vm_fast_links` / `vm_global_ics`, indexed by *per-process* site ids) are
per-process: every green process compiles its own copy of each prelude function it calls.
The source/AST is shared via the PRELUDE/RUNTIME regions; the **compiled** form is not.

Confirmed by scaling with the number of distinct prelude functions a unit calls (N=20k,
`(mem-bytes)`, exact live bytes): 0 fns 13,786 B/proc; 1 (`fold`) 31,619; 2 (`+count`)
49,954; 3 (`+map`) 59,914; 4 (`+filter`) 66,503 — roughly +6-18 KB per distinct callee.

Ruled out, so don't re-chase: the `PARK_TRIM_GROWTH_SLOTS = 64` threshold (rebuilt with it
at 0 — 32.72 → 31.72 KB/proc, noise); JIT inlining (`BROOD_NO_JIT=1` unchanged at 32.63);
worker count (flat 33.5 KB/proc from J=2 to J=12, so not cross-thread page fragmentation);
allocator retention (glibc arena knobs are no-ops — the allocator is **mimalloc**, and
`mallinfo2` only sees the main arena, which is why it read a flat 67 KB while RSS was
675 MB). `(mem-bytes)` is the right probe: it reported 31,618 B/proc against 674 MB RSS,
94% accounted, so the memory is genuinely live, not fragmentation.

Not fixed. The direction is to share compiled closures across processes the way the
RUNTIME region already shares AST (keyed by closure handle + epoch); the IC tables must
stay per-process, but they are only ~3.5 KB of the ~18 KB, and could be sparse rather
than dense `Vec`s indexed by site id.

**Benchmark harness** (`../brood-benchmarks`) gained the per-CPU columns this motivated:
`cores` (CPU% / 100) and `CPU·s` (wall × cores), because wall time alone cannot tell
"fast" from "cannot scale". On `spawn` at N=200k: Node runs at **102% CPU — one core**, a
ceiling no machine lifts, while .NET's 804% is genuine thread-pool parallelism (its fast
result was not a rigged port). Brood posts the best utilisation of the four (925%) and
still loses on wall, burning 8.05 CPU-s to Node's 1.23 for identical work — the gap is
per-core compute, not parallelism.

That column also caught a measurement bug that had been flattering Python: 3.14 defaults
multiprocessing to `forkserver`, so `pfib`'s pool workers were children of the forkserver
and never reaped, and `/usr/bin/time` read 4% CPU on a run using 10.6 cores — ranking
Python the *most* CPU-efficient runtime in the suite. Pinned the port to the `fork` context
(1055% CPU, 26.07 CPU-s — the worst on the row) and added a guard that marks any row under
50% CPU on a >200 ms run as under-reported and excludes it from the ranking.

## 2026-07-28 (cont.) — decomposing `spawn-live`: what the 4 s is actually made of

The published row is Brood's worst (5.35 s / 4.45 GB at 300k, last of five). Decomposed
at N=300,000, so the next attempt starts from evidence rather than the headline:

| unit does | wall | RSS |
|---|---|---|
| spawn + park, never woken | 2.28 s | 6.11 KB/proc |
| + wake, reply, parent collects (no prelude call) | 2.88 s | 8.47 KB/proc |
| + one `(fold + 0 p)` (the real row) | 4.04 s | 15.2 KB/proc |

So per-process compiled code (ADR-175) is worth **+1.16 s and +2.0 GB** — real, but only
~29% of the time. The rest is a **2.88 s / 8.5 KB-per-process floor** for spawning, waking
and reaping 300k processes, against Elixir's 2.5 µs and ~3.1 KB. Fixing ADR-175 alone lands
at ~2.9 s / 2.55 GB, still ~4× the BEAM. Both halves need work; neither alone closes it.

Counterintuitive and worth remembering: **spawn-and-park (2.28 s) is slower than
spawn-wake-exit (1.43 s)**. `spawn-live` deliberately holds all 300k alive, so nothing is
reclaimed and allocator/cache pressure dominates. The row measures *residency*, so
per-process footprint is the lever, not per-spawn CPU.

**Dead end 1 — "cache the capture-free spawn thunk" is already implemented.** `spawn` does
`heap.promote(f)` on its thunk, which appends a closure (plus captured env) to the shared
append-only RUNTIME region, and at 300k live processes the ADR-091 collector can reclaim
none of it. That looked like an easy win until reading `Heap::closure_const_cache`, whose
doc names the exact case: a `(fn …)` with no lexical captures and no self-name is a
constant, memoised once and promoted to a stable RUNTIME handle — *"`(spawn (worker))` in a
fan-out drops ~7×"*. It works: capture-free spawn costs 6.14 KB/proc and capturing one int
costs 7.13 KB, i.e. the ~1 KB delta is the capturing case paying for a real promoted env
frame, not waste. **Do not re-propose this.**

**Dead end 2 — per-phase allocation attribution via `live_bytes()` does not work.**
`crate::core::alloc::live_bytes()` is process-wide across every worker thread, so deltas
taken around phases of one `spawn` are contaminated by concurrent allocation on other
workers (the samples came back including wrapped negatives). Whole-program differencing
(spawn N, divide) is sound; per-call-site differencing is not. Attributing the remainder
needs a real allocation profile.

**Still unaccounted: ~2.9 KB of a parked process's 5,597 measured live bytes.** Identified:
`Box<Process>` 1736 (with `Heap` 1648 inline), `Arc<Mailbox>` ~184 (`Mailbox` 168 /
`MailboxState` 112 / `Message` 40), `Suspended` 128, slabs ~480, roots ~128, ICs ~40,
registry ~50 — about 2.7 KB. The rest is unidentified; the earlier guess that it was
per-spawn closure minting was wrong (see dead end 1).

## 2026-07-28 (cont.) — rejected: skipping `live_vm_arms` registration for PRELUDE consts

**Tried, measured, reverted. Do not re-attempt without new evidence.**

`node_has_rt_handles` marks an arm as needing `live_vm_arms` registration if its body
holds any `ConstVal::Handle`. A **PRELUDE** handle is immovable — the prelude is a
separate immutable region `runtime_collect` never compacts, and `ConstVal`'s own doc says
"PRELUDE handles never actually move (the flush is a no-op for them)". So registering such
an arm looked like pure waste, and skipping it should have skipped an `Arc::clone` on the
hot call path plus the cross-worker refcount contention the flag exists to avoid.

It does not. Measured with `make ab` (best-of-7, then best-of-11 solo on the movers):

| row | delta |
|---|---|
| fib / bintree / nqueens / sort / json / collatz | +2.6% … −0.7% (flat) |
| **spawn** | **+6.3%** (solo, best-of-11) |
| **ring** | **+4.0%** (solo, best-of-11) |

No gain anywhere, a real cost on the process rows. Reverted.

**Calibration worth keeping:** after reverting, an A/B of functionally identical binaries
still read `spawn` **+3.1%**. That row's noise floor is ~3%, so treat anything under ~5%
there as unproven — and the +6.3% above was a smaller real effect than the number suggests.

**The reasoning error, for next time:** "this skips work on the hot path, so it must be
faster" is a hypothesis, not a result. The registration presumably pays for something
(`arm_slot` is threaded through `BcFrame` and the JIT paths), so removing it moves cost
rather than deleting it. `make ab` is the gate precisely because plausible reasoning about
this VM has been wrong repeatedly.

## 2026-07-28 (later) — The unkillable nested eval: found, fixed, REPL Ctrl-C on by default

The morning entry ended with "root cause not located" and a wrong suspect
(`charge_native`). Both corrections first, because the *misdiagnosis* is the useful
part: the original four-site fix was aimed at real gaps, but the loop under test
never ticked at any of them — and my instrumentation "proving" that was misread.
The debug output had three line families; I grepped for one and piped the live run
through `head`, so 2,800 `plain tick()` rollovers sat unexamined in the capture
file while I concluded plain `tick()` had fired once. Re-counting the same file
found them immediately. Diagnosis lesson: count every line family in the *whole*
capture before concluding a path is dead.

Those 2,800 rollovers pointed at the one `tick()` site never patched:
`passthrough_redirect_ok`. That's the real mechanism — in a tree-walked loop like
`(defn spin (n) (if (> n 0) (spin (- n 1)) :done))`, the operators `>` and `-` are
thin-wrapper passthroughs (Brood defns over `%gt`/`%sub`, ADR-069), so the
reduction budget drains in the `'dispatch` redirect gate, ~2 ticks per iteration,
starving the loop-top safepoint of rollovers. The function's own docstring records
the eval *deadline* escaping through this exact gap once. Same hole, third
occupant: preemption fairness never needed a check there (`preempt()` just refreshes
the budget wherever it fires), the deadline did and got one, and the kill check is
now the second resident.

The fix (ADR-176): `tick_reporting_hard_kill()` — `tick()` plus, on the rollover
only, a pending-hard-kill probe — at all five non-capturing safepoints (tree-walker
loop top, the passthrough gate, `exec_chunk` ×2, `vm_run_bc`). On `true` the site
unwinds with the pre-existing untrappable `Control::Kill`, and
`handle_capture_outcome` gains the kill-signal conversion so a tree-walked body
retires with the mailbox reason instead of crashing as an uncaught error. Hot path
unchanged (the probe is once per ~2000-reduction quantum); root thread constant-false.

Verified: the three-route repro all `DIED reason=:kill` on default / `BROOD_NO_JIT=1`
/ `BROOD_VM=0`; kill latency 4ms on a loop killed *after* 3s of JIT tiering;
untrappable through `try`; soft exit still deferred inside eval; clean under
`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`; four new cases in `tests/exit_test.blsp`
(12/12 that file — the full suite runs on the other machine).

With the kernel honest, the REPL flipped on: interactive sessions arm SIGINT and run
each form in a spawned process; Ctrl-C kills it (`; interrupted`, prompt back, image
+ `*1` + child-made `def`s intact — pty-verified), second Ctrl-C halts (exit 130) for
an eval wedged in one native call. Piped sessions unchanged. Off-switch:
`(def *repl-interruptible* false)` in `.broodrc.blsp` — the knob is the ambient
global itself, resolved to the armed state after the rc loads (so the rc opt-out
short-circuits the handler install), not an env var.

## 2026-07-28 (review pass) — REPL hardening: nested sessions, interrupt ownership, meta-command gaps

A critical review of the day's REPL work, driven by actually composing the pieces —
in particular with `pry` (std/tool/debug.blsp), which launches `repl-run` *inside* a
session and therefore exercises every seam at once. Five real defects, all fixed and
pty-verified; the printer came out clean (lazy ranges print bounded without realising;
records are maps so they bound for free; the 30s "printing hang" suspect turned out to
be debug-build *eval* cost — `(do (map inc (range 0 3e7)) :made)` is just as slow).

1. **pry's `debug>` prompt was lost.** `(binding (*repl-prompt* "debug> ") (repl-run))`
   binds a dynamic, but the loop runs in a spawned process and dynamics don't cross
   `spawn` — the nested prompt silently fell back to `brood> `. `repl-run` now captures
   the prompts in the caller's extent and re-`binding`s them inside the spawn body.
2. **Interrupt theft between nested sessions.** Outer and nested loops both polled the
   one read-and-clear flag every 40ms; whoever won consumed the Ctrl-C, and an outer
   win would hard-kill its child — the entire pry — when the user meant to stop one
   expression at the `debug>` prompt. (Trials favoured the inner by phase luck, but the
   race is architectural.) Ownership is now explicit: runners register in a stack (a
   `Table`, the sanctioned shared-mutable store — strict nesting means push/pop never
   race) and only the innermost live owner polls; a dead head (a runner killed rather
   than returning) is repaired past, so ownership can't wedge on a corpse.
3. **A script that pried lost its Ctrl-C forever.** `repl-run` installed the SIGINT
   handler and never removed it, so after pry returned, Ctrl-C set a flag nobody polls
   — the script became uninterruptible. New `%restore-interrupt-handler` primitive
   (SIG_DFL, the uninstall half of the seam), and the *outermost armed* `repl-run`
   owns restoration via a token: pry-under-REPL leaves the outer handler alone,
   pry-under-script restores fatal Ctrl-C on exit. Verified both ways.
4. **The rc re-ran on every nested session** — side effects re-executed, defs
   re-applied mid-session. Once-per-runtime latch.
5. **`,time`/`,type` evaluated user code inline** — uninterruptible (Ctrl-C during
   `,time` of the exact slow expression it exists for would set a flag nobody reads),
   and `,time` printed unbounded. Both now run through the same interruptible runner
   as ordinary forms (`repl--call`), and `,time` prints bounded. `,doc` also stopped
   splicing raw input into code text — `,doc foo) (halt 0) (` would have *evaluated*
   the injected forms (eval-string reads all, then evals all); the argument must now
   read as exactly one symbol.

Also: the earlier auto-indent change had silently broken lineedit_test's "Enter inserts
a newline" case (it now inserts an *indented* newline — intended, but the file was
never re-run after that change; process lesson repeated: run the owning test file after
every behaviour change, not just the new feature's cases). Test updated to pin the new
behaviour. Full sweep green: repl 14, exit 12, lineedit 56; pty scenarios — nested
prompt, nested-eval Ctrl-C, `,time` Ctrl-C, outer-after-nested Ctrl-C, rc-once,
script-pry restore, completion listing, auto-indent, bounded print, `*1` — all pass.

## 2026-07-29 — ADR-175 Phases A+B land: shared compiled code, spawn-live −37%

**Phase A — arm-relative IC site ids.** Sites number from 0 within each compiled arm;
each process lazily resolves a contiguous IC block per arm it activates
(`Heap::vm_arm_block`, keyed by a process-independent arm uid) and the drivers install
the running arm's block as heap cursors at every transition (call/tail/return/resume/
suspend-capture; `vm_apply`/`vm_resume_deopt` as save/restore chokepoints; dispatch's
callee-JIT path before `push_frame` so optional defaults index the callee's block; the
JIT's direct native entries; the HOF per-element fast frame). Zero JIT IR changes —
`brood_rt_fastlink_base` returns a block-adjusted base/len. Perf: all 8 A/B rows within
noise (max +3.0% on `spawn`, at its floor). Also fixes a live degradation: shared native
code previously ran with the compiling process's absolute site ids, failing every other
process's bounds checks — ICs silently dead in installers.

**Phase B — runtime-shared compiled closures** (the BEAM module-area move). PRELUDE
closure handle → `Arc<CompiledClosure>` in `RuntimeCode`, `jit_code_cache` pattern.
Strict gate: PRELUDE-region keys only (never freed/recycled — no ADR-091 free-epoch
discipline needed) and **immortal** arms (no RUNTIME-region handle anywhere, checked by
running `rewrite_arm_handles` with a recording identity fn — the one authoritative walk
— so two processes can never double-forward a shared arm; PRELUDE handles are immovable
so the rewriter is identity on them). `BROOD_NO_SHARED_ARMS=1` is the off-switch.

Measured (N=300k spawn-live; per-proc figures at N=100k):

| | before | after |
|---|---|---|
| spawn-live wall | 4.04 s | **3.32 s (−18%)** |
| spawn-live RSS | 4.57 GB | **2.86 GB (−37%)** |
| fold-unit live bytes | 32.7 KB/proc | **21.1 (−35%)** |
| user-code body (body_big) | 37.1 | 37.5 (unchanged — user arms not shared yet) |

Suite: 883/883 after fixing a stale `lineedit` assertion (the new auto-indent feature
inserts `\n  `; the pre-feature test expected bare `\n` — updated to the feature's
documented contract, "two past the opening paren's column").

**Open regression, documented not hidden: `collatz` +8.0% (solo, best-of-11).** The
off-switch recovers it (110→103 ms) and it vanishes under `BROOD_NO_JIT` (+1.2%, noise)
— so it is JIT-tiering-state persistence: a shared arm shares `jit_calls`/`jit_deopts`/
`compile_epoch`, changing when native code runs vs the per-process cold start (a
re-installed shared arm keeps its tiering history where a recompile used to reset it —
`sort` moved −5.2% by the same mechanism, in the winning direction). Fix direction:
split the shared code (body/chunk/shape) from per-process tier state. Until then the
trade is +8% on one row against −1.7 GB at 300k processes.

**Next (not started): share RUNTIME-keyed user arms** — the remaining ~30 KB/proc in
body_big-class workloads. Needs free-epoch discipline in the shared map (RUNTIME keys
recycle) and either idempotent-rewrite proof or the immortality gate generalised.

## 2026-07-29 — `std/` adopts abilities across the tree (ADR-177)

A second, deliberately aggressive pass over `std/` + the prelude for ability candidates.
The 2026-07-28 audit (with ADR-171) found only two and called the rest "correctly-closed
`cond`". That was too narrow: it asked only "where does third-party extension want in?",
which is one of four reasons a site wants an ability. Full rationale + the rejection list
in **ADR-177**; what shipped:

**Abilities (6).**
- **`JsonEncode`/`to-json`** (`std/json.blsp`) — the ADR-171 follow-up. A record picks its
  wire shape; a pid/fn/datetime becomes encodable by impl'ing instead of hitting
  `json: cannot encode`. **No `:default`** on purpose (it would turn the loud error into
  infinite recursion). Bonus speedup: the old code ran an O(n) `remove :__id__` filter over
  *every* map; plain maps now skip it (`record?` first).
- **`Dependency`** (`std/tool/package.blsp`), `:sealed` over four dep records now in
  `std/tool/project.blsp` — replaces **five** scattered `(get dep :kind)` chains (resolve,
  compatibility, lock row, manifest entry, tree label). Verified the payoff directly: drop
  an op and `nest check` reports `sealed ability … no impl of \`X\` for :…`. A resolved
  entry is its dep record + resolution fields `assoc`'d, so one ability covers dep *and*
  entry. The tree label became the `Display` impl, so `(println dep)` and `nest tree` can't
  drift.
- **`Port`/`io-write`** (`std/io.blsp`) — the module docstring already asked for this
  ("room to grow a port into a richer value … without touching callers"). `(impl Port :fn)`
  keeps every fn port working; `standard-port`/`process-port`/`file-port` are records that
  print as `#<port stdout>`. `*out*`/`*err*` still hold a bare fn (the prelude calls it
  directly — no dispatch on the print path); `port-fn` adapts a record at that boundary and
  `with-out`/`with-err` apply it.
- **`LogBackend`/`backend-emit`** (`std/log.blsp`) — a backend goes from "a map whose
  `:format` fn you may replace" to a value owning its whole write policy; `backend-passes?`
  is the reusable level/filter gate. `file-backend` now uses `io`'s `file-port`.
- **`Response`/`send-response`** (`std/net/http.blsp`) — replaces the server's
  `(contains? resp :stream)` branch. The two kinds differ in *who closes the socket*, so
  that lives with the type. Deliberately open (a sendfile / chunked / 101-upgrade response
  is a `defrecord` + `impl` elsewhere).
- **`Temporal`/`to-iso`** (`std/datetime.blsp`), `:sealed` over the three temporal types —
  collapses `date->iso8601`/`time->iso8601`/`dt->iso8601`, three functions the caller chose
  between by knowing which shape it held.

**Records for plain maps that were identified by sniffing (5).** `buffer` (was
`(and (map? x) (rope? (get x :rope)))` — a map with a real rope passed), `queue`, `pq`,
`multimap`, `datetime`/`date`/`time-of-day`. Each gains an unfoolable predicate and a print
form that isn't its internal representation. Two findings worth keeping:
- **`pq` fixes a live footgun.** It was a bare list, and `()` ≡ `nil` is the one falsy
  collection — so `(if pq …)` was silently false for an *empty* queue and true otherwise.
  A record is always truthy.
- **`multimap` had to WRAP, not become, the map.** A record carries `:__id__` as a real
  field, so a record-as-map would have leaked it out of `multimap-keys` and counted it in
  `multimap-empty?`. Wrapping keeps the inner map pristine. (`buffer` is fine unwrapped —
  nothing calls `keys`/`count` on one.)

**Tier 3.** `(str v)` → `(to-str v)` at three policy-layer call sites — `std/template`
(substitutions), `std/csv` (cells), `std/url` (query values) — so a record renders as its
own display string. Consistent with ADR-171 keeping `str` itself native. The csv one also
fixes a latent bug: a non-string cell used to reach `includes?` and raise a type error.

**Suite.** 3762 tests green in release (+~60 new). Test updates were the honest cost of the
change: a record is never `=` to a look-alike map, so `project_test` / `package_test` /
`datetime_test` map-literal comparisons became constructor comparisons. New coverage
includes cross-process blocks for every value-carrying conversion (a record deep-copies on
`send` and keeps its `:__id__`, so `*impls*` dispatch works identically at both ends).

**Two process lessons.**
1. **`cargo build --bin brood` does not rebuild `nest`.** std modules are `include_str!`'d,
   so a std edit needs *both* binaries rebuilt. Mid-session I dropped `--bin nest`, and 40
   `nest test` failures looked like a concurrency race (nondeterministic counts across runs)
   when they were just a stale embedded std. Same family as the 2026-06-18 `-p brood` A/B
   trap: **rebuild every binary that embeds what you edited.**
2. **The 3 KI-14 failures in the debug suite are timeouts, not regressions** (120s cap;
   6–18s each in release). Only `json-parse` is involved — untouched by the encoder change.

**Pre-existing flake noticed, not fixed.** `tests/ability_test.blsp` is the *only* file that
registers impls at test-*run* time (~20 `impl`/`register-impl` calls inside `test` bodies,
in `:serial` groups). Those race with each other on `*impls*`'s read-modify-write — the
caveat `language.md` documents ("two processes calling `impl` concurrently can lose one
update"). One full-suite run lost the `Size`/`:vector` registration; the next was green.
The fix is to make those groups `:isolated` (which runs a unit alone) rather than
`:serial` — left alone as out of scope, since `:isolated` also rolls globals back.

## 2026-07-29 (impl) — the collection protocol, read half: a `Seqable` ability

`seq`/`count`/`keys`/`vals` dispatched via a closed `cond` over built-in collection kinds
with no extension point — and, since unified `defrecord` made every record carry `:__id__`,
they *leaked* it: `(count (point 1 2))` was 3, `(keys r)` included `:__id__`, `(map f r)`
iterated the id. Fixed both at once with a `Seqable` ability (op `to-seq`), hybrid à la
`Display`: `seq` keeps every built-in's native path and, for a RECORD only (one `:__id__`
check), takes its `Seqable` view — the fields id-free by default (`(map-pairs (fields x))`),
or whatever a custom impl returns. So `map`/`filter`/`fold`/`for`/`into` (which all coerce
through `seq`) and `count`/`keys`/`vals` now give a record its clean field view, and a
custom-collection record — `(impl Seqable stack (to-seq [s] (get s :items)))` — joins the
protocol: `seq`/`count`/`fold`/`map`/`filter` all iterate its items.

Bootstrap-order care: `seq` is defined before `map?`/`cond`/the ability machinery, so it
detects a record with the raw `map-get` builtin and calls `to-seq` **late-bound** — only
ever reached for a record, which never exists at boot, so the forward reference is safe.
The `:__id__` check is once per collection *operation* (seq/count are called once, not
per element), so built-ins and plain maps pay effectively nothing.

Verified: record 15 (incl. custom-Seqable + leak-fix tests), ability 34, show 12, json 41,
maps 83 — green, GC-stress clean. Still to come (the user wants the full suite): the
**build/lookup half** — `Conjable` (`conj`/`into`) and `Lookup` (`get`/`nth`) for custom
collections — then the `Ord`/`Compare` ability (sort/min/max on records) and eventually a
numeric protocol.

## 2026-07-29 (impl) — collection protocol, build half + dogfooded onto std (queue/multimap)

Added `Conjable` (op `-conj`), the build counterpart to `Seqable`: `conj`/`into` (both
prelude defns, so no hot-path cost) dispatch for a RECORD — default is the map behaviour
(assoc a `[k v]`, merge a map), a custom collection defines its own insertion. Same
bootstrap-safe raw `map-get :__id__` gate as `seq`; `-conj` is only reached for records.

Then dogfooded the whole protocol onto std's own collection types — the actual cleanup:
`std/queue` and `std/multimap` now `(impl Seqable …)` (and queue `(impl Conjable …)`), so a
queue/multimap is a **first-class collection**: `count`/`seq`/`map`/`filter`/`fold`/`for`/
`into` (and `conj` for the queue) all work on it directly. Their bespoke functions
collapsed onto the protocol — `queue-to-list` → `(seq q)`, `queue-from-list` →
`(into (queue-new) xs)`, `multimap-size` → `(count mm)` — turning a parallel API into thin
aliases. (pq/multimap take no `Conjable`: inserting needs a priority/key a bare `conj`
can't carry — `pq-insert`/`multimap-assoc` stay.)

Deferred: the Prim1 accessors `first`/`rest`/`empty?`/`nth` don't route through `Seqable` —
they're JIT-inlined ops the hot `fold--loop` uses, so routing them safely needs kernel work
(a general-builtin record branch, or raw `%first`/`%rest`); `(first (seq c))` meanwhile.

Also chased down the recurring "KI-14 flakes" — they were **not real**: the canary
(`tests/jit_deep_recursion_test.blsp`) parses a 100k-deep JSON in a spawned process with a
60s `receive` timeout; run via `cargo run -p nest` (a **debug** build, ~10× slower) under
full-suite load it exceeds 60s → `:TIMED-OUT`. On release (`make test` / `nest test`) it is
2.9 s, 3/3. Lesson: verify the suite on release, not the debug `cargo run -p nest`.

Verified (release): queue 27, multimap 20, record 16 (incl. custom Seqable+Conjable),
ability 34, maps 83, json 41; full release suite clean.
## 2026-07-29 (cont.) — ADR-175 Phase C: user-code arms share too

Phase B shared only PRELUDE closures. Phase C extends the runtime-shared cache to every
shared-region closure (PRELUDE + RUNTIME), so **user code compiles once per runtime**
instead of once per process. LOCAL closures stay unshared (recycled handles, movable
embedded handles).

**The blocker was misdiagnosed, in our favour.** The recorded reason RUNTIME keys were
excluded was "two processes rewriting one shared arm would double-forward its handles".
Reading the compactor shows that is *impossible*: `runtime_collect` runs only under
`Arc::get_mut` on the runtime — i.e. when this is the sole process. The real hazard is the
mirror image: compaction rewrites arms on the **execution stack** (`live_vm_arms`), so a
merely *cached* shared arm is on no stack, is never visited, and keeps pre-compaction
handles. Fixed by clearing the shared cache where the compactor already clears `vm_cache`.
The second hazard — a freed generation recycling handles bit-identically (ADR-091) — is
handled by stamping each entry with the `free_epoch` its publisher observed **before**
compiling and validating on lookup, so a free mid-compile leaves the entry stale rather
than poisonous. `closure_is_immortal` is deleted: it guarded the impossible hazard and
excluded essentially all user code (whose constants are promoted to RUNTIME).

Measured (N=300k spawn-live; per-proc at N=100k):

| | pre-ADR-175 | Phase B | **Phase C** |
|---|---|---|---|
| spawn-live wall | 4.04 s | 3.47 s | **3.00 s** |
| spawn-live RSS | 4.57 GB | 2.80 GB | **2.01 GB** |
| 40-arm user body | 37.1 KB/proc | 37.5 | **4.55** |
| trivial body | 6.27 KB/proc | 6.78 | **4.52** |

`body_big` converged onto `body_small` exactly as predicted, and both fell *below* the old
6.27 KB floor — small bodies were being duplicated too. Same-binary off-switch A/B at 300k:
3.00 s / 2.01 GB shared vs 4.91 s / 4.33 GB with `BROOD_NO_SHARED_ARMS=1`.

Gate: suite 883/883, stress 33/33. `make ab` vs Phase B: **`spawn` −21.7%**, `ring` −5.2%,
everything else flat — sharing is a speed win as well as a memory one.

**Open, unchanged in kind: shared JIT-tier state.** A shared arm shares
`jit_calls`/`jit_deopts`/`compile_epoch`, so tiering history persists across installs where
a per-process recompile reset it. Cumulative `make ab` vs pre-ADR-175 (`e6ab599b`):
`spawn` −14.8%, `ring` −3.9%, `fib` −1.3%, but `nqueens` **+7.8%** and `collatz` **+4.9%**
(both solo-confirmed). Attribution for `nqueens`: 111 ms shared vs 107 ms with the
off-switch, and **with `BROOD_NO_JIT=1` sharing is slightly *faster*** (298 vs 302 ms) —
so the cost is tiering history, not the code sharing. Fix direction unchanged: split the
shared code (body/chunk/shape) from per-process tier state. Trade as it stands: ~5–8% on
two compute rows against −2.6 GB and −1 s at 300k processes.

## 2026-07-29 (impl) — `Ord` ability: a record defines its own sort order

Added `Ord` (op `compare-to` → -1/0/1). `sort`/`sort-by` (prelude defns) now compare
through `ord-compare` — a record's `Ord` impl if it has one, else the kernel `compare` —
so a version / money / card record sorts by a meaningful order instead of its arbitrary
(but deterministic) structural map order. Hybrid as ever: the kernel `compare`/`%sort-asc`/
`%sort-cmp` stay native for built-ins; only a record element/key routes through `Ord`
(`sort` gains one `(record? (first a))` branch). Default `compare-to` is the structural
`compare`, so records without a custom order still sort deterministically. Verified:
record 18 (incl. semantic-version ordering + built-in-sort-unchanged), maps 83, no
regression. Remaining of "lean into abilities": the numeric protocol (`+`/`-`/`*` for
records) — the highest-risk one, since it touches the hottest paths.

## 2026-07-29 (finding) — numeric protocol for records: Brood-side is a ~195× fib regression

Tried a `Num` ability so records (money, complex, vectors) could use `+`/`-`/`*`/`/`, wired
by a `(record? a)` branch in each operator's binary arm (`(if (record? a) (num-add a b)
(%add a b))`), int/float falling straight to `%add`. It works — money arithmetic dispatches,
ints/floats give the right answers — but the one mandatory `make ab`-style check killed it:
**fib 35 went 60 ms → 11.7 s, ~195×.** A `(record? a)` branch, however cheap in isolation,
makes `+` non-trivial enough that the JIT can no longer lower `(+ a b)` to a native int-add;
the whole recursion falls back to slow interpreted calls. Reverted.

Lesson (already in CLAUDE.md, now with a number): arithmetic operators are pure JIT
substrate — *any* Brood-level branch in them is catastrophic. A numeric protocol has to be
a kernel change: dispatch `Num` only from the `%add`/`%sub` *fallback* (operands not already
numeric) or a JIT type-deopt, leaving the inlined path untouched — plus checker work to
accept a `Num` record operand. Filed as a maybe-item in ROADMAP; the collection + `Ord`
protocols (which don't touch the numeric hot path) shipped fine. Also confirmed the type
checker is already clean for every shipped construct (Seqable/Conjable/Ord/Display): `nest
check` is 0 warnings on the tree and on fresh user code — the `+` warnings were purely this
reverted numeric change.

## 2026-07-29 (impl) — numeric protocol, done properly in the kernel (zero regression)

The Brood-side `Num` was a ~195× fib regression (a `(record? a)` branch defeats the JIT's
arithmetic specialization). The kernel form has **zero** cost: the `%add`/`%sub`/`%mul`/
`%div` builtins dispatch `Num` only from their COLD non-numeric fallback — a new
`num_record_dispatch` checks if the first operand is a record (a `Value::Map` carrying
`:__id__`), and if so applies the matching `num-*` ability op via `apply_value`; otherwise
it returns `None` and the builtin proceeds to its float/error path. The int/float hot path
is untouched — the JIT inlines int+int / float+float and never calls the `%add` builtin, so
it never reaches the fallback. Measured: **fib 35 = 61 ms**, identical to baseline (60 ms).
The operators stay `(%add a b)` — no Brood branch, so nothing to defeat the JIT.

The `Num` ability (`num-add`/`num-sub`/`num-mul`/`num-div`, no `:default`) lives in the
prelude; a record with no impl raises the ability's loud missing-impl error. A money value
does `(+ (usd 500) (usd 250))` → `750`, variadic `+` folds through the same dispatch.

Checker: `+`/`-`/`*`/`/` widened from `number` to `number | map` (a record is a map), so
`(+ money money)` and `(get (+ a b) :field)` type-check. Precision is preserved — the
structural `numeric_call_ty` types a pure-numeric call as int/float and only DEFERS to the
curated sig once an operand is a record, so the widened sig affects record arithmetic only;
`(+ "a" 1)` is still caught (a string isn't `number|map`). Verified: fib 61 ms (zero
regression), float loop unaffected, record 20 (incl. Num arithmetic + int/float-untouched),
math 81, decimal 20, maps 83; `nest check` 0 warnings on the tree AND on `(+ money money)`.
This completes the "lean into abilities" arc — collection (read+build), `Ord`, and now `Num`.
## 2026-07-29 (cont.) — the ADR-175 "regression" was a `make ab` artifact

The `collatz` +8% / `nqueens` +7.8% attributed to shared JIT-tier state in the two
entries above **is not a real cost**, and the planned "split shared code from
per-process tier state" fix would have solved nothing while undoing `spawn` −14.8%.

Bisect first: the regression is Phase B (prelude sharing), not Phase C — `collatz`
+7.9% at the Phase A→B boundary, −0.9% at B→C. Then `BROOD_JIT_DUMP_IR` counts:
sharing lowers **18 arms vs 7**, and the extra ones are prelude helpers (`not`, `fold`,
`cond--orphan`, `fold--loop`) that never reach the threshold otherwise. That is the
mechanism working as intended — hotness accumulates across the runtime instead of
resetting per process, so more prelude code tiers up. It is *why* `spawn` gained 14.8%.

The cost is paying for those compiles. `make ab` pins compute rows to **one core**
(`AB_PIN_CPU`, default cpu2), so the background JIT compiler thread competes with the
benchmark for that core; 11 extra compiles at ~0.7 ms each ≈ the 8 ms "regression".
Give the compiler its own core and it vanishes:

| collatz | 1-core pinned | unpinned |
|---|---|---|
| shared | 110 ms | 103 ms |
| `BROOD_NO_SHARED_ARMS=1` | 102 ms | 104 ms |

`nqueens` behaves the same, and under the **harness's** actual pinning (cores 8-11) both
rows show no regression — so the published cross-language numbers were never affected.

**Methodology lesson, now in CLAUDE.md:** `make ab`'s single-core pin is right for
measuring generated-code quality and wrong for any change that alters *how much*
background compilation happens — it charges the benchmark for compiler CPU that a real
run does in parallel. Re-run such a change unpinned before believing a regression.

## 2026-07-29 (cont.) — the green-process floor, finally attributed

The `spawn-live` gap is now entirely the process floor (compiled code is shared per
runtime since ADR-175). Roughly half of it was unattributed, and the earlier attempts
failed for a recorded reason: whole-program differencing is exhausted, and per-phase
`live_bytes()` deltas are invalid because that counter is process-wide across workers.

Done properly with a **size-class histogram in the `Counting` global allocator**
(temporary; env-armed from `main`, since an env read *inside* the allocator re-enters
it). One trap worth writing down: dumping at `atexit` reads almost empty — `Interp::drop`
retires parked processes first, so the interesting allocations are already gone. Dump
while the interpreter is alive (right after `run_files`).

Diffing N=50,000 parked processes against N=0: **15.8 live allocations and ~4.8 KB per
process.**

| size class | per proc | bytes/proc | what |
|---|---|---|---|
| ≤2048 | 1.00 | 2048 | `Box<Process>` (1840 B) |
| **≤256** | **6.92** | **1773** | `Arc<Mailbox>` (184 B) + the `Heap`'s slab/root `Vec`s |
| ≤128 | 2.96 | 379 | smaller `Vec` first-allocations |
| ≤512 | 0.99 | 506 | |
| ≤16/32/64 | ~3.9 | 142 | |

The ≤256 cluster is the lever, and its cause is mechanical: `Value` is 24 B and
`(Value,Value)` 48 B, so Rust's minimum non-zero `Vec` capacity (4 for these element
sizes) makes every first-touched slab `Vec` a 192 B allocation — in the 256 class. A
process touching ~6 of the 11 slab kinds pays ~6 × 192 B before storing anything.

Fix directions, in rough order of payoff:
1. **Arena the slabs** — one backing allocation for all 11 `Vec`s instead of one each.
   Collapsing ~7 allocations into 1–2 is ~1 KB/proc against a 4.8 KB floor.
2. **Explicit small initial capacity** (`Vec::with_capacity(1)` = 48 B, class 64) where a
   slab kind is usually near-empty — cheaper to try, smaller win.
3. `Process` is 1840 B, up from 1736 before ADR-175 Phase A (the IC block registry +
   cursors). Still one 2048-class allocation, so it costs nothing extra today, but it is
   ~200 B from the next class boundary.

Not implemented — this entry is the measurement. The BEAM's ~3 KB against our ~4.8 KB of
*allocation* (≈6.6 KB RSS) is now a concrete, itemised target rather than a mystery.

## 2026-07-29 (cont.) — two attempts at the process floor, both reverted

Following the allocation profile above, two cheap levers were tried against the ~4.5 KB
parked-process floor. Both measured, both reverted; the profile's *attribution* is
corrected as a result.

**1. Park-trim threshold.** `trim_parked` collects + `shrink_to_fit`s a parking process's
slabs, but `PARK_TRIM_GROWTH_SLOTS = 64` skips it for a process that allocated little —
exactly the `spawn-live` shape. Rebuilt with the threshold at 0 (always trim):
**4.59 → 4.60 KB/proc**, i.e. nothing. (Same result as the first time this was tried, when
compiled code still dominated — so it is not that the win was hidden then.)

**2. Capacity-1 first touch in `alloc_slot!`.** Rust's `RawVec` rounds a first push up to
capacity 4, so a 48-byte `(Value, Value)` slab allocates 192 B before holding anything;
forcing `reserve_exact(1)` should have saved ~144 B per touched slab. Predicted ~700
B/proc. **Measured 4.52 → 4.41 KB/proc — 110 B**, and it cost `bintree` **+4.8%** (solo,
best-of-11): allocation-heavy code pays the extra 1→2→4 reallocs. Bad trade, reverted.

**The prediction being 6× off is the useful result** — the slab `Vec`s are *not* the bulk
of the 6.92 allocations/proc in the 129–256 B class. Working back from the measured sizes,
the likely composition is `Arc<Mailbox>` (184 B), `Suspended.frames` (4 × 64 = 256 B),
`roots` (8 × 24 = 192 B), and **four from the per-process IC tables** — `vm_call_ics`,
`vm_fast_links`, `vm_global_ics` and the `arm_ic_blocks` registry. That last group is
ADR-175 Phase A's, and it is now the largest identified item in the floor.

Worth stating plainly: Phase A did **not** make the floor worse — measured against the
pre-ADR-175 binary the floor went **6.27 → 4.53 KB/proc**, because sharing removed far
more than the IC tables added. But those tables are where the next attempt should look,
and the honest next step is *per-allocation-site* attribution (backtraces), not another
guess from size classes. The arena-the-slabs idea from the previous entry is now
lower-priority than it looked: slab `Vec`s account for ~110 B/proc, not ~1 KB.

## 2026-07-29 (cont.) — the process floor is working state, not waste

Per-structure attribution of a parked process (probe in `trim_parked`, since size-class
profiling had gone as far as it could):

| structure | bytes | |
|---|---|---|
| `vm_call_ics` | 384 | cap 4 × 96 B (`Option<CallIcEntry>`) |
| `vm_fast_links` | 160 | cap 4 × 40 B |
| `arm_ic_blocks` | 120 | 2 entries |
| `roots` | 192 | |
| slabs | 256 | |
| `live_vm_arms` | 32 | |
| **per-process tables** | **1144** | plus `Box<Process>` 1840, `Arc<Mailbox>` 184, `Suspended` 128 |

So the **inline-cache tables are 664 B** — the largest single identified item, and the one
worth attacking. They are pure caches (every entry validated on `(sym, argc, epoch)`), so
dropping them is always safe; `runtime_collect` already does exactly that.

**Dropping them on park works, and costs too much.** Measured:

| policy | parked floor | spawn-live | pingpong | ring |
|---|---|---|---|---|
| baseline | 4.53 KB/proc | 2.00 GB | — | — |
| drop on every park | 3.89 | 1.75 GB | **+26.1%** | **+18.1%** |
| drop on **first** park only | 3.91 | 1.74 GB | **+11.5%** | **+16.9%** |

The first-park heuristic barely helps, which is the informative part: the cost is not the
*frequency* of dropping but that a process loses the caches it built during startup and
has to rebuild them at the start of its hot loop. Both reverted — 0.64 KB/proc is not
worth 12-17% on the message-latency rows, which are already the widest gap to Elixir.

**Conclusion for this line of work.** Three attempts, all measured, all reverted:
park-trim threshold (nothing), capacity-1 slab first-touch (110 B, `bintree` +4.8%), IC
table drop (640 B, `pingpong`/`ring` +12-17%). The pattern is consistent: **the ~4.5 KB is
working state a process genuinely uses, not slack.** Getting to the BEAM's ~3 KB needs the
state to be *smaller*, not dropped — e.g. shrinking `CallIcEntry` (96 B, of which the
`fast` memo is 32) or sharing IC entries for frozen callees across processes (ADR-175
Stage 3, sound because a sealed binding's resolution is process-independent). Those are
design changes, not tuning, and should be costed before being attempted.

## 2026-07-29 (cont.) — the runtime option book: docs/runtime-frontier.md

Wrote the full analysis-and-menu for the remaining runtime gaps to the BEAM —
per-process anatomy as measured this week, how ERTS/Go/Pony structure the same problems,
and every option with precedent, expected win, and risk. Key verified facts driving it:
local `send` is TWO full copies through the wire-format `Message` (BEAM copies once,
directly into the receiver's heap); a BEAM process is one memory block (heap up, stack
down) with no per-process code state at all (the export table is the shared, always-warm
"IC"); X registers are scheduler-owned; hibernation is opt-in — ERTS hit the exact
tradeoff our reverted IC-drop hit and gave it to the programmer.

Execution order chosen: (hibernate) builtin → process-shell recycling → single-copy send
to a parked receiver → direct-link sealed callees → cold-heap split. The list lives in
the doc; items get ticked or moved to its dead-ends section as they're measured.

## 2026-07-29 (cont.) — defability hardening: variadic ops, collisions, polymorphic dispatch

Reviewed the ability system (`defability`/`impl`, ADR-172) and fixed a batch of
correctness/ergonomic bugs found by probing the release binary, plus 7 stale checker
tests the fix surfaced. All in `std/prelude.blsp`, `crates/lisp/src/types/check/protocol.rs`,
`crates/lisp/src/core/heap/vm_cache.rs`, `crates/lisp/src/types/display.rs`.

- **Variadic ops were silently broken.** `(op [self & args])` emitted a *direct* call
  `(impl self & args)`, so `&` was passed as an argument — an unbound-symbol error at every
  call, with no diagnostic at declaration. The op now emits `(apply impl self … rest)` for a
  `&`-rest arg (`defability--op-call`).
- **Zero-arg ops** (`(op [])`) dispatched every value on `(identity-of nil)` → the `:nil`
  impl (no polymorphism, no error). Now rejected at macro-expansion.
- **`record?` disagreed with dispatch.** It used `contains?`, so a hand-written
  `{:__id__ nil …}` was a "record" to `record?`/`record-id`/`fields` yet dispatched as
  `:map` (`identity-of` requires a *truthy* id). `record?` now requires a truthy id.
- **Op-name collisions silently clobbered.** Two abilities declaring the same op name in one
  ns overwrite each other's generic `defn`. `register-ability` now warns — guarded by
  `bound?` so it fires only when a real generic function is shadowed (crafted precedence
  fixtures using bare `register-ability` stay quiet).
- **Impl arity** is validated at registration (`register-impl--check-arity`): a warning when
  an impl's arity is incompatible with the declared op, for known abilities (a fixed op skips
  when its own spec is variadic).
- **`no-impl`** now throws a structured `{:kind :no-impl :message :ability :op :id :have}`
  instead of a bare string, so a handler can branch on the parts; `error-message` still
  returns the same human text.
- **Dispatch inline cache is now polymorphic** (`DISPATCH_IC_WAYS = 4`, ADR-172 §7). The
  monomorphic cache re-resolved on every call for an op applied over mixed identities in one
  loop (`to-str`/`inspect` over a heterogeneous collection); it now memoises up to 4 `id→fn`
  associations per op under one epoch guard, round-robin eviction when full.
- **Checker:** an op-fn symbol two different abilities bind is marked ambiguous and dropped
  from the static missing-impl pass (no false-warn, no false-pass); `parse_op` understands
  `&`, so a variadic impl of a fixed op no longer false-warns on arity.
- **`number`-alias display.** Records doing arithmetic via the `Num` ability widened the
  operators' argument domain to `number | map`; since `{int,float,decimal,map}` ≠ `Ty::NUMBER`
  the printer spelled out `int | float | map | decimal`, breaking 7 checker tests that assert
  the message names `number`. The printer now factors the `number` alias out of any strict
  pure-tag superset, so it reads `number | map` — clearer messages *and* the tests pass.

Full suite green (`make test`: 883 passed); `tests/ability_test.blsp` extended with variadic,
structured-error, and polymorphic-cache (>4 ids) coverage.

## 2026-07-29 (cont.) — ability ergonomics review: single-dispatch scope, no cross-type arithmetic, op-name uniqueness

A design pass over the shipped ability system (`defability`/`impl`, ADR-172) asking "is
this ergonomic, how does it compare to other ecosystems?" — landing three decisions that
kept the language surface *unchanged* (no new syntax, no new forms). Recorded under ADR-172
as amendment 2026-07-29b.

- **`defmulti` considered and declined (for now).** The one thing single-dispatch can't
  express is multiple/structural dispatch. But the entire shipped ability surface is
  single-dispatch, nominal record dispatch already covers Clojure's main `defmulti` use
  (dispatch on a map tag), and structural-map dispatch is *deliberately* closed by ADR-011.
  That leaves only cross-type binary methods — declined below. So a Clojure-style
  `defmulti`/`defmethod` seam is deferred with an explicit trigger rather than built (ADR-011
  "don't pay the two-mechanisms tax until a concrete need appears").
- **No implicit cross-type arithmetic.** `Num`/`Ord` are homogeneous single-dispatch. Fixed
  the one visible footgun: `(+ 5 (money 50))` fell through `num_bin` to `num_to_f64` and died
  with "expected number, got map". It now raises a named error ("a record operand must come
  first — `Num` dispatches on the first argument … cross-type arithmetic is not implicit").
  One `is_record` guard in `crates/lisp/src/builtins/numeric.rs`; `(+ money money)` unchanged.
- **Op names are unique per module, now ship-blocking.** The checker already computed the
  same-module op-name collision as its `ambiguous` set but only used it to suppress false
  positives. It now *also* emits a diagnostic (`check_op_collisions` in
  `types/check/protocol.rs`, wired in `check.rs`), so `nest check` rejects it (advisory in
  the live image, per ADR-123–126). The runtime `register-ability` warning stays as the
  cross-file backstop. No `ability-call`/`Shape/area` escape valve — greenfield renames.
- **Doc drift fixed.** `language.md`'s `impl`-id note still called the bare/qualified
  asymmetry an open issue; KI-15 fixed it on 2026-07-27, so the note now describes the shared
  `ability--id-kw` behavior.

Docs: `docs/language.md` (single-dispatch/binary-op note, op-name-uniqueness note,
nominal-only on-ramp), `docs/decisions.md` (ADR-172 amendment 2026-07-29b). Tests:
`tests/ability_test.blsp` ("Num is homogeneous single-dispatch"),
`crates/lisp/tests/type_check_catalog.rs` (op-name uniqueness warn + two false-positive
guards).

## 2026-07-29 (cont.) — L1: the single-copy local send, and a fold that kept the source form

Two things, one of which was a bug in already-pushed work.

**The bug first.** `make test` came back with 3 failures — quoted-symbol patterns falling
through to the catch-all, and `'foo` dependency names arriving as "must be a symbol, got
(quote foo)". They were **not** from the change in flight; a clean build of HEAD in a
worktree reproduced them, which is the check worth doing before assuming your own diff did
it. The cause was the all-constant vector-literal fold added with the receive tag filter
(`eb983671`): it promoted the **raw source form**, but an element of a vector literal is a
value-position *expression*. `[:tag 'go]` reads as `[:tag (quote go)]`, so the folded
constant kept the list where the symbol belonged. Now it folds only when every element
*evaluates to itself* — its compiled constant is structurally the source element — which
covers the self-evaluating literals the optimisation was for (including the `receive` tag
vector it was added for) and excludes anything evaluation changes. The general fix would
build the constant from the compiled values, but `compile_node` holds only `&Heap`.
Regression coverage added to `tests/vectors_test.blsp`, since both suites that caught it
did so incidentally. Suite green again, pushed on its own as `be08019e`/`5e416438`.

**L1 (ADR-178).** A `send` to a *parked* local process now copies the value straight from
the sender's heap into the receiver's, skipping the `Value → Message → Value` round trip.
The access is licensed by an ownership fact the mailbox already maintains: a parked process
*is* its `Box<Process>` in `MailboxState::waiter`, so taking it under the mutex confers
exclusive `&mut` on its heap. Anything not provably safe — a running receiver, a remote
pid, a value the copier declines — falls through to the unchanged wire path.

What the measurements actually said, including where they contradicted the plan:

- **The win scales with payload**, because what's removed is marshalling, not scheduling.
  Pinned, best-of-5, same commit: −3.2% empty, −9.0% at 16, −24.2% at 64, −31.3% at 256,
  −34.9% at 1024.
- **The benchmark rows can't show it.** `ring` sends a bare int; half of `pingpong`'s
  messages are a bare keyword. `pingpong` ≈ −5%, `ring` within drift, everything else flat.
  That's the suite's message shapes, not the change — recorded in the ADR so it isn't
  re-derived backwards later.
- **The "receivers aren't parked" theory for `ring` was wrong.** Added `BROOD_L1_STATS=1`
  rather than keep guessing: the fast path hits **100%** on both rows.
- **`spawn` +5.9%** on the first cut — a 24-byte `Vec` inline on every `Heap`, which is
  inline in `Box<Process>`, where bytes cost ~2:1 in RSS via mimalloc size classes.
  Lazily boxing it to 8 bytes took `spawn` back to flat and kept the payload curve.

Gate: suite green, TSAN clean, `BROOD_GC_STRESS=1` and `BROOD_GC_VERIFY=1` clean. New
`crates/lisp/tests/local_send_race.rs` (added to `make tsan`) attacks the ownership claim
where the existing TSAN tests didn't — many senders racing structured payloads into one
repeatedly-parking receiver, checked by value so a dropped or corrupted delivery moves the
total, plus a mixed fast-path/declined-closure variant that proves a decline restores the
receiver untouched.

## 2026-07-29 (cont.) — M2, and the duplicate fast-link it turned up first

Went to cost M2 (shared IC tables) and came back with a blocker and a shipped win.

**The blocker.** `vm_fast_links_base()` hands JIT'd code a *raw pointer* into the fast-link
table, sound only under `SAFETY: single-threaded per process`. Sharing that table across a
runtime's processes doesn't just need a lock — a peer growing it reallocates under a live
raw pointer held by running native code. So M2 needs **stable-address per-arm blocks**
(`boxcar` is the in-tree precedent), and runtime-global base assignment can't ship ahead of
the shared table: a process touches ~4 sites, so runtime-global bases with per-process
tables would size every process to the runtime's whole site count. Recorded in
`runtime-frontier.md` as M2b so the next attempt starts from the real design.

**The win it turned up.** `CallIcEntry.fast` (32 B) and the `FastLink` mirror held the same
fact — the source comment already said "Same data, written in lockstep". `FastLink` carries
`sym`/`argc`/`epoch` too, so the VM probe never needed the 96-byte entry on a hit; it now
reads the same 40-byte flat table JIT'd code reads. One representation, one write.

- **−157 B per live process**: spawn-live 1.94 → 1.89 GB (−47 MB over 300k processes).
- Compute rows *improve*: pfib −3.5%, collatz −2.7%, fib −1.3%, bintree −0.8%, nqueens
  flat, spawn-live −0.8%. `spawn` +1.9%. Dropping the memo was expected to cost the hot
  recursive call an extra table touch; it didn't — one 40-byte flat slot beats a 96-byte
  entry load.
- It also makes M2b cheaper: what's worth sharing is now unambiguously a `#[repr(C)]`
  plain-data slot, not an entry containing a `Cell`.

Method, again: the sweep said `spawn` +5.8% / `collatz` +2.8%; solo re-runs said +1.9% and
**−2.7%**. Two false regressions in one day — solo-confirm anything under ~5%.

Gate: suite 3797 green, TSAN 0 warnings, eval fuzzer 84k runs, `make stress` 33/33 across
jit / no-jit / gc-stress / chaos.

## 2026-07-29 (cont.) — typed ability ops: the checker consumes `:-> RET` (ADR-180)

Opened as "improve the type system in light of the ability changes." The intersection is
where the win was: abilities give records a nominal identity and the checker already bridged
into it (record shapes, `ty_record_id`, missing-impl warnings, sealed exhaustiveness), but
an ability op — the polymorphic boundary — was **type-opaque both ways**. The op spec's
`:-> RET` return tail had been declared pervasively across `std/` + `tests/` since abilities
shipped and was **parsed and thrown away**.

So slice 1 is pure checker, no grammar/runtime change:

- **Call-site flow.** `AbilityInfo` now parses each op's `:-> RET` into `op_ret: (ability,
  op) → Ty` (from this file's `register-ability` forms *and* the `*abilities*` registry, so
  cross-module returns are visible). `infer::expr_ty` returns it for a call on a known op
  head, so `(area s)` for `(area [self] :-> float)` types as `float` — `(string-length (area
  s))` is now flagged where it was silent.
- **Impl conformance.** `walk::check_impl_returns` grades each `(register-impl … (fn …))`
  body's last form against the op's declared return, reusing `gradual_of`/`consistent_with`
  — the same false-positive-clean machinery as the ADR-110 `sig` return check. An `impl`
  returning a string where the op says `:-> int` warns; an unknown/`number` body defers; a
  `:-> any` op imposes nothing.

Gate: `types::check` 195 green (+4 new), ability suite 42 → 46 (+4 in-language, via
`check-string-structured`), and the safety check that matters — `nest check` over **all** of
`std/` + `tests/` shows **zero** return-type warnings (the declarations are dense there, so
this is the real false-positive test). Runtime dispatch byte-for-byte unchanged. Recorded in
ADR-180; language.md §Polymorphism updated. Next: slice 2 (nominal sum types + `match`
exhaustiveness over sealed abilities), then slice 3 (`BROOD_MONO` Tier 1 devirtualization).

## 2026-07-29 (cont.) — multiple dispatch: `defmulti`/`defmethod`, and `Num`/`Ord` move to it

Single-dispatch abilities cannot express a binary operator symmetrically (`(+ money 5)` can
dispatch on `money`; `(+ 5 money)` cannot dispatch on the second arg). Built the deferred
`defmulti` seam (ADR-179) and migrated `Num`/`Ord` onto it.

- **Mechanism (`std/prelude.blsp`).** `defmulti`/`defmethod` dispatch on the identity-tuple of
  the args, resolved by exact match → a single `:default` → a loud structured `no-method`.
  Unambiguous by construction (no partial wildcards). A `:commutative`/`:antisymmetric` algebra
  derives each off-diagonal binary method's mirror (`(f y x)` / `(- 0 (f y x))`), so a binary op
  is authored as the upper triangle of its type matrix; an authored method always wins over a
  derived one. STRICT: arg-count≠pattern-length, a closure on a non-binary pattern, an unknown
  algebra, a method for an undeclared multi, and `:default` as a tuple position all fail at load.
- **`Num`/`Ord` migrated.** They were `defability`s; now `(defmulti num-add :commutative)` etc.
  and `(defmulti compare-to :antisymmetric)`. No implicit coercion — `(+ money 5)` needs an
  authored `[money :int]` method; commutativity then makes `(+ 5 money)` agree.
- **Kernel (`builtins/numeric.rs`, `mod.rs`).** `+`/`-`/`*`/`/` and `<`/`<=`/`>`/`>=`/`min`/`max`
  route to the multimethods **only when a record is an operand** (the cold fallback where a
  `record?` operand lands); pure int/float/decimal stays byte-for-byte on the fast path. The
  old first-operand-only `num_record_dispatch` became a both-operand `num_multi_dispatch`, plus
  a new `compare_multi_dispatch` for the comparison/min/max path.
- **Checker (`types/check/sigs.rs`, `builtins/mod.rs`).** Widened `< <= > >=` (curated) and
  `min`/`max` (their **primitive** sig, which outranks curated) to `number | record`, so
  `(< (usd 1) (usd 2))` type-checks clean.
- **Tests.** `tests/multimethod_test.blsp` (7, incl. cross-process); `tests/record_test.blsp`
  Num/Ord via `defmethod` (usd*int scaling, commutative mirror, a `no-method` case, the
  `{:__id__ nil}` truthy-id edge). Full suite green (885 passed).

Known follow-ups: a `nest check` static-coverage warning for a *statically-known* missing
method (abilities have `check_ability_calls`; multimethods don't yet), and a `make ab` pass for
the two `is_record` tag-compares added to the float-comparison cold fallback.

## 2026-07-29 (cont.) — a sealed ability is a type (ADR-181); soundness audit

Slice 2 of the type-system-meets-abilities work. ADR-180's `:-> RET` raised "what else can
an op return?" and "how do I type a function over an ability's domain?" — both need the
ability *name* to be a type. So: in `annot::parse_type`, a bare symbol naming a **sealed**
ability resolves to the union of its members' record shapes (`Shape :sealed [circle rect]` →
`(or (record :__id__ :ns/circle) (record :__id__ :ns/rect))`). `(sig total (Shape ->
float))` and `:-> Shape` now typecheck. Open abilities deliberately stay non-types (no finite
member set) — sound-not-complete.

Mechanism mirrors the `SIG_MEMO` pattern: a per-file thread-local `ABILITY_TYPES` populated
before sig parsing from `protocol::sealed_member_ids` (this file's expanded `register-sealed`
forms, where member ids are already ns-qualified, ∪ the runtime `*sealed*` registry), cleared
per file *and* on the ad-hoc `(check 'form)` path so nothing leaks between checks.

**Soundness audit (the user's explicit ask — "1000% sound, needn't be complete").** The
call-argument check grades a precise arg (map literal, sig-typed param) with `⊆` and a
dynamic arg (constructor call, inferred var) with `∩ ≠ ⊥`. The strict `⊆` path is where a
record-in-union subtyping bug would surface, so it was tested head-on: a map-literal member,
a `Shape`-typed param, and a `(circle 2)` call all pass clean; only a provable non-record
(`int`) warns; a plain `map` defers (an unrefined map isn't provably-not-a-Shape). Then the
definitive check — `nest check` over **all** of `std/` + `tests/`: every warning emitted is
the pre-existing non-tail-recursion advisory, **zero** from types/abilities/returns/args
across both slices. Slice 1 audited the same way: the impl-return and call-flow paths reuse
ADR-110's gradual machinery, and on a *valid* program (impls honor their declared returns)
neither can false-positive.

Gate: `types::check` 264 green (+5 over slice 1's 259), ability suite 49 (+3 in-language).
ADR-181; language.md + roadmap updated. Next: slice 3 — `BROOD_MONO` Tier 1 (off-by-default
syntactic devirtualization); flag-off must be byte-identical, flag-on suite-green.

## 2026-07-29 (cont.) — ability-dispatch monomorphization, Tier 1 (ADR-182); mono soundness

Slice 3: the runtime companion to the two checker slices. Where the checker *proves* a
dispatch identity, the compiler can *use* it. Behind `BROOD_MONO` (off by default), an
ability op call with a **literal first arg** (`(size 5)`) is rewritten at the `compile_node`
seam to a *direct* call to the resolved impl fn (`Node::Const(impl_fn)` callee), skipping the
op body's `identity-of` + two `impl-for` CHAMP fetches. `mono_devirtualize`
(`eval/compile/inline.rs`) mirrors runtime dispatch exactly — identity = the literal's
`type-of` kind, impl = `*impls*[[ability op]]` by id then `:default`.

**Soundness, two guarantees, both verified.** (1) Flag off = provably inert: the only cost is
one cached `mono_enabled()` bool, so default builds are byte-for-byte unchanged (ability suite
identical off vs on). (2) Flag on = correct + GC-safe: every uncertainty (arg not a literal,
head not a registered op, a map literal that could be a nominal record, no impl for the id)
*declines* the rewrite, so a missing impl still raises the same `no-impl`; verified
byte-identical across `:int`/`:string`/`:default`/record-ctor/no-impl shapes, `BROOD_MONO_DBG`
confirmed the rewrite fires exactly where expected; and the baked impl fn is a promoted RUNTIME
handle, so `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` under the flag is clean (no use-after-GC,
stored-handle verifier included).

The late-binding trade-off (a captured impl goes stale on re-registration) is the *documented*
reason it is flag-gated — not a bug. Tier 1 is literal-only (proves the mechanism; doesn't move
hot loops); Tier 2 (inferred-variable devirt via a checker→compiler channel — the real win and
the real miscompile surface) and the direct-constructor extension stay deferred with
whole-fleet validation. ADR-182; CLAUDE.md flag table + ability-monomorphization.md updated.

**All three slices land the "type-system-meets-abilities" arc:** typed op returns (ADR-180) →
sealed ability as a type (ADR-181) → devirtualize the proven dispatch (ADR-182). Checker
soundness held throughout — zero false positives across `std/` + `tests/`.
## 2026-07-29 (cont.) — 288 bytes of checker state off every process, and a regression that wasn't

**The change.** Two pieces of `Heap` were inline-and-hot-by-accident but cold-by-use:

- `check_dep_rec` + `module_exports_cache` + `known_ns_cache` → one lazily-boxed
  `CheckHeap` (288 B → 16 B). `check_dep_rec` alone was **208 bytes** — four `HashSet`s —
  on every process, when only `nest check` ever touches it. M1 left these behind because
  they are all filled through `&self`, so they couldn't use `ColdHeap`'s
  `Option<Box<_>>` shape; putting the box behind a `RefCell` and handing out a
  `RefMut::map` guard solves exactly that blocker.
- `dbg_site_pos` gated to `#[cfg(debug_assertions)]`. Every *use* was already gated; the
  field was not, so release carried 32 B of provably dead weight per process.

`Heap` 1616 → 1376 B (−15%). Measured **−47 MB on spawn-live** (1.894 → 1.847 GB,
−157 B/process — the 240 B struct shrink rounds through mimalloc's size classes). Checker
verified intact: zero warnings project-wide including a forced `BROOD_NO_CHECK_CACHE=1`
full recheck, and a deliberately bad file still warns.

**The regression that wasn't, which is the more useful half.** `make ab` reported
`pingpong` +4.3% in a sweep, then **+5.3% on a solo re-run** — by the rule we'd written
down, a confirmed regression. It wasn't. The row's `make ab` *baseline* had drifted
209 → 230 ms across the day's invocations (~10%), so the solo "confirmation" measured
drift a second time.

Two wrong hypotheses got chased before the measurement was fixed, both worth recording as
dead ends: it is *not* an extra indirection on a hot path (`rec_check_dep_*` is called only
from `types::check::deps`, never from running code — checked, not assumed), and the
bisect that seemed to split the cost between the two pieces (+2.3% for the box alone) was
itself noise.

The method that actually answers it: a fixed baseline binary plus a **base-vs-base
control** — `taskset`-pinned best-of-15 for base, base again, then new, reading the
base-vs-base spread as the row's noise floor. Controlled: fib 0.0% (floor 0.0%), spawn
−0.9% (floor −0.9%), bintree +0.8% (floor +1.6%), pingpong +0.9% (floor +0.5%), spawn-live
−1.2% (floor +0.4%). Neutral on time, real on memory. Note added to CLAUDE.md.

## 2026-07-29 (cont.) — BROOD_MONO Tier 1: the direct-constructor shape (ADR-182)

Completed Tier 1 by adding the second syntactic shape the checker's `arg_identity` already
proves: a **direct record-constructor call** as the ability op's first arg — `(area (circle
2))` devirtualizes to circle's impl, alongside the literal shape (`(size 5)`) shipped earlier.

The soundness crux was proving, *at compile time*, that a call head is a genuine record
constructor and what id it bakes. A record id keyword's symbol *is* the qualified constructor
name (`:m/circle` ← `m/circle`), so identity is `keyword(ctor)` — but that alone can't tell a
record constructor from a same-named plain fn. Trusting the constructor's declared `sig`
(which carries a `(record :__id__ …)` return) is **unsound** — a sig is an unchecked contract
that can lie, and a wrong devirt is a *miscompile*, not a false warning. So `defrecord` now
populates a ground-truth `*record-ids*` registry (id-keyword → record name); mono devirtualizes
only when `keyword(ctor) ∈ *record-ids*`. `(area (circleish 5))` — a non-record fn — is
rejected and stays dynamic. Verified byte-identical (circle/rect → nominal impls; circleish →
`:default`), and 49→51 ability tests green off, on, and under `BROOD_GC_STRESS=1
BROOD_GC_VERIFY=1` (the baked impl fn is still a promoted RUNTIME handle).

The registry is cheap always-on metadata (one `assoc` per `defrecord` at load, like `*impls*`)
and independently useful; it does not touch the compile-time inert-when-off guarantee. Tier 2
(inferred-*variable* devirt) stays deferred. ADR-182 / ability-monomorphization.md / CLAUDE.md
updated.

## 2026-07-29 (cont.) — `Ord` made strict, like `Num` (no structural `:default`)

Follow-up to ADR-179. `Num` had no `:default` (a record pair with no `num-*` method is a loud
`no-method`), but `Ord`'s `compare-to` kept a `:default` = kernel structural `compare`, so a
record silently ordered by its map layout and `(< money 5)` returned a structural answer
instead of erroring. Dropped it: `Ord` is now strict too — a record type must define
`compare-to` to be ordered (`<`/`<=`/`>`/`>=`/`sort`), else `no-method`. Impact audit first:
no `std/` site sorts bare records — every `sort`/`sort-by` there is over scalars (strings,
numbers, symbols) or `[k v]` vectors (kernel `compare`), so nothing regressed. Full suite green
(894). `std/prelude.blsp` + language.md updated.

## 2026-07-29 (cont.) — BROOD_MONO benchmarked against dynamic dispatch

Measured the Tier-1 devirtualization (ADR-182) against dynamic ability dispatch. It's a
runtime-flag A/B on ONE binary (toggle `BROOD_MONO`), so `make ab` (which compares two
*builds*) doesn't apply — release+JIT binary, 5M dispatch calls in a tight tail loop, one
op call per iteration, min-of-N. Correctness checked first: dynamic and mono print
byte-identical results.

| Dispatch shape | VM-only (`BROOD_NO_JIT=1`) | JIT-on (unpinned) |
|---|---|---|
| **Literal** `(sz 7)` | 2.67 → 1.01s = **2.6×** | 2.85 → 0.50s = **~5.7×** |
| **Constructor** `(ar (circle 7))` | 6.58 → 3.52s = **1.9×** | 7.17 → 3.96s = **1.8×** |
| Plain call (control, no ability) | — | 0.14 → 0.15s = **1.0×** (no-op) |

Findings:
- **Literal dispatch is the big win (~5.7× with the JIT).** Dynamic barely improves from VM
  (2.67s) to JIT (2.85s) — the op body (`identity-of` + `%dispatch` + branch) resists JIT
  optimization. Devirt turns it into a direct impl call the JIT *can* optimize (→0.50s), so
  mono's value here is as much "unblocks the JIT" as "skips the two CHAMP lookups."
- **Constructor dispatch wins less (~1.8×)** — the per-iteration `(circle 7)` record
  allocation is a fixed cost both paths pay, so the dispatch saving is a smaller slice.
- **Control is a no-op** (1.0× within noise) — mono touches nothing without ability dispatch.

Caveats (why this doesn't move the standard rows): it's a best-case microbenchmark whose loop
body is *only* dispatch; real code does work per call, so whole-program speedup is smaller. And
Tier 1 fires only on **literal / direct-constructor** args — the common hot-loop shape
`(map area shapes)` (a *variable* arg) is Tier 2, not devirtualized here — so on the standard
benchmark rows the impact is ~zero, exactly as the design note predicted.

Method note (relearned): single-core `taskset` pinning was wildly unreliable here — the
background JIT compiler contends on the pinned core, giving a 6.74s→1.02s swing between runs
of the *same* dynamic config. VM-only isolates the raw dispatch cost; unpinned gives the
real-world number; the two agree in direction and magnitude. Same lesson as CLAUDE.md's
"re-run unpinned when a change touches compilation volume."
## 2026-07-29 (cont.) — the old generation is empty for 99.998% of processes

`Heap` carried `old: Slabs` inline — eleven `Vec` headers, 264 B — on every process. A
process only populates an old generation by surviving a minor collection, and a short-lived
worker never collects at all. Measured before writing any of it: **7 of 300,000**
spawn-live processes ever promote. So it is now `Option<Box<Slabs>>` (8 B), allocated on
first promotion.

The shape that makes it safe: reads go through `old()`, callable only where an OLD handle is
already in hand — and an OLD handle can only exist if a promotion allocated the slabs, so
the `expect` is unreachable by construction. Aggregate walks use `old_opt()` and tolerate
absence. Mutation goes through `old_mut()`, which allocates.

**−73 MB on spawn-live** (1.847 → 1.774 GB), spawn-live −1.7%.

**It costs `fib` +2.6%, and that is real** — measured against a ~1% base spread, and it
survives the layout control below. `fib` promotes heavily, so it pays the extra indirection
on OLD-handle derefs (and on the two JIT old-slab base-pointer shims). Shipped anyway: the
process floor is the `spawn-live` gap, which is our worst row, and 73 MB at 300k processes
outweighs 2.6% on one compute row. Reverting is a contained change if that judgement is
wrong.

Two process notes worth keeping:

- **A blanket regex over 91 `self.old` sites broke the collector**, and only `BROOD_GC_VERIFY`
  caught it: `verify_local_graph`, the trace stats and the JIT base-pointer shims all run at
  *every* collection, but the rewrite gave them `old()`, which `expect`s an old generation
  that usually doesn't exist. Base completed the probe in <120 s; the new binary timed out.
  The lesson is that "reads use `old()`" is only true for handle derefs — aggregate walks
  are a different category and needed `old_opt()`.
- **A padding control separates layout from mechanism.** `sort` first read +2.7%; restoring
  the removed 256 B as dead padding took it to +0.0%, so that was struct-layout shift, not
  the change — and indeed the final unpadded measurement has `sort` at −0.6%. `fib` kept
  +2.6% under padding, which is how we know its cost is the indirection. Any change that
  shrinks a hot struct should run this control before believing a nearby row moved.

Gate: suite 3804 green, `make stress` 33/33, TSAN 0 warnings, GC_STRESS and GC_VERIFY clean
(including a probe that deliberately populates and re-reads an old generation).

## 2026-07-29 (cont.) — multimethod static-coverage check (the ADR-179 follow-up)

Discharged the deferred follow-up: `nest check` now flags a direct `defmulti` generic call
whose FULL argument tuple is statically known (every arg a literal or a `defrecord` ctor call)
but has no exact method and no `:default` — the multimethod analogue of `check_ability_calls`,
so an unclear dispatch fails at type-check, not only at runtime.

- `types/check/protocol.rs`: `build_multi_info` (recognises a generic by its `multi-resolve`
  body fingerprint — sound, like the ability `%dispatch` fingerprint; collects methods from
  this file's `register-method` forms + the runtime `*methods*` registry) and `check_multi_calls`.
- **Closure mirrors accounted for:** a `:commutative`/`:antisymmetric` `[A B]` method also
  covers `[B A]`, mirroring the runtime's `register-method--derive`, so `(scale 3 money)` for a
  `[money :int]` method does NOT false-warn. This was a real false positive caught in testing.
- Only judges a call all of whose args have a certain identity — one unknown arg (a variable)
  defers. No inference hook yet (record-typed *variables* aren't judged); syntactic-only, sound.
- Scope: fires on a **direct** generic call (`(num-add …)`, a user `(defmulti mine)` call), not
  on the `+`/`<` operator sugar (which the checker doesn't see through to `num-add`).

Zero false positives across `std/` + `tests/` (full suite 894 green); clippy + fmt clean.
Tests: `type_check_catalog.rs` (a miss warns; covered/mirror/`:default`/unknown-arg stay silent).

## 2026-07-29 (cont.) — typed ability op parameters `(name T)` (ADR-180 follow-on)

Completed the typed-abilities story: an op could already declare what it *returns* (`:-> RET`,
ADR-180); now it can declare what it *accepts*. `(scale [self (factor float)] :-> int)` wraps
a param as `(name T)` — the argument-side sibling of the return type.

Runtime: one line in `defability` — strip the type from the generated op `defn`
(`(factor float)` → `factor`), so it stays a checker-only annotation and dispatch is unchanged
(still on the first arg's identity; `(arglist scale)` → `(self factor)`). Checker: parse the
param vector's `(name T)` entries into `AbilityInfo.op_params` (same two sources as `op_ret`),
then (1) check each argument at a typed position at the call site — `(scale s "x")` warns
"argument 2 expects float" — reusing the exact sig-param gradual relation (`gradual_of` +
`consistent_with` + `relax_param_for_arg`), and (2) bind the impl body's param at that type so
returning it against a disjoint `:-> RET` is caught.

False-positive-clean by construction (untyped positions — every bare param and `self` —
impose nothing, so all existing all-bare op specs are inert): full type suite 264 → 268,
ability suite 51 → 54 (+3 in-language, incl. the soundness case that an unknown-typed arg
DEFERS), and **zero** new argument/return warnings across all of `std/` + `tests/`. ADR-180's
deferred item (a) discharged; language.md updated. This wraps the type-system arc — typed
returns + typed params (ADR-180), sealed-ability-as-a-type (ADR-181), devirtualization
(ADR-182).

## 2026-07-29 (cont.) — multimethod check: inference hook + operator-sugar coverage

Extended the ADR-179 static-coverage check two ways, both sound (zero false positives across
`std/` + `tests/`, full suite 898 green; clippy + fmt clean).

- **Inference hook** (`check_multi_call_inferred`, wired via `Ctx::multi()`, mirroring the
  ability `check_ability_call_inferred`). A direct generic call at least one of whose args is a
  *symbol* resolves each arg's identity syntactically OR from its inferred **record type**, so
  `(let (m (usd 1)) (scale m 2.5))` is flagged. The symbol gate prevents double-warning with the
  syntactic pass (a fully-literal call has no symbol arg).
- **Operator-sugar coverage** (`check_operator_sugar`). `+`/`-`/`*`/`/` → `num-*` and
  `<`/`<=`/`>`/`>=` → `compare-to` when a record is an operand: `(+ (usd 1) 2.5)` /
  `(< money 5)` are flagged when the routed multimethod has no method for the pair. A record
  operand is *required* — told apart from a number via the runtime `*record-ids*` registry
  (ADR-182) — so pure `(+ 1 2)` is never touched. 2-arg only (a variadic fold's intermediate
  type is unknown); the antisymmetric mirror makes `<`/`>` direction irrelevant.

The deliberate runtime-error tests stay silent for free: an operator no-method test lives inside
`try` (which `check_into` skips), and a direct-generic one uses a variable. `protocol.rs`,
`walk.rs`, `check.rs`, `ctx.rs`, `type_check_catalog.rs`.

## 2026-07-29 (cont.) — the `seqable` type + recursion inference (making types pay off)

Two moves to make the type system *do more* rather than build more machinery, prompted by a
sig-adoption pilot (std carries sigs on ~1% of its defns — the checker only helps where types
are declared).

**A `seqable` type (`Ty::SEQABLE`).** A `sig` had no way to say "any seqable of T": `(list T)`
false-flags a vector caller, so a polymorphic-sequence parameter had to fall back to `any` (no
checking). Added `seqable` to the grammar — the named union `nil | pair | vector | set | map |
bytes` (a range/seqview reads as `pair`; `string` excluded, matching runtime seqability), the
same domain the curated combinator sigs already used internally, now nameable with a clean
`Display`. `write-lines`/`fuzzy-filter`'s seq params moved off `any` to `seqable`: a
vector/list/set/map caller passes, a non-seqable (`5`, `"abc"`) is flagged.

**Recursion inference.** The inferencer (`sigs::infer_sig`) gave up on any recursive function
— a self-call's type is unknown (cycle), so the return deferred, and callers of the ~ubiquitous
`--acc`/`--loop` helpers got no checking. Fix: when inferring a function's own signature, a
self-recursive call in a *branch-result* position contributes ⊥ to the return union (a new
`Ctx::inferring_self`, consumed in `infer::branch_union`). Sound by induction — the recursive
branch returns exactly the fixpoint the base cases already define, so skipping it lets the
return infer from the base cases. A `count-down` returning `:done` now flows to its callers;
an accumulator-returning recursion (unknown base case) still defers, never false-flags.

The cardinal-sin gate held both times: type suite 268 → 270, and **zero** new false positives
across all of `std/` + `tests/`. Next lever on inference: multi-arity/rest closures, and the
bottom-up fixpoint order so a caller of an as-yet-un-inferred function still resolves.

## 2026-07-29 (cont.) — provided ops: default method bodies in `defability` (ADR-185)

Comparing Brood's ability/dispatch stack to the most-loved languages, the dispatch *core*
(open extension, precedence tiers, multiple dispatch with operator algebra, hot reload) is
ahead of the pack — but one loved ergonomic was missing: **provided methods** (Rust traits,
Swift protocol extensions, Haskell typeclasses — implement the required ops, inherit the
derived ones). Built it.

**The change.** An op spec may now carry a **body** after its optional `:-> RET`:
`(op [args] :-> ret? body…)`. `defability` registers that body as the op's `:default` impl
(from the declaring ns → ability-owner tier). A bodyless spec stays a **required** op.
`(defability Ord (compare-to [self other] :-> int) (lt [self other] :-> bool (< (compare-to
self other) 0)) …)` — an `impl` writes only `compare-to` and inherits `lt`/`gt`/…; an
id-keyed impl of a provided op overrides it (id key beats `:default`).

**Why it's small.** Dispatch already falls back id-impl → `:default`, so the generated
generic function and the dispatch path are **unchanged** — a provided op is just an
auto-registered impl, and the inline cache / precedence / hot reload / cross-process paths
all carry over. No new special form, no `Value` kind, no builtin: a prelude macro change
(`defability--op-body` + a `fold` emitting the `register-impl` forms) plus a checker
adjustment. Specs are stored *with* bodies in `*abilities*` so the checker can tell provided
from required.

**Checker.** Two advisory passes learned "provided" (`spec_has_body` + an `Op.provided`
flag / an `AbilityInfo.provided` set): per-`impl` completeness no longer demands a provided
op, and `:sealed` exhaustiveness no longer demands it of a member — but a **required** op is
still demanded in both. `nest check` stays zero-warning across `std/` + `tests/`.

**Wrinkle worth noting.** Each `impl` *form* is completeness-checked on its own, so
*overriding* a provided op is done by adding its method to that type's existing `impl` (same
form as the required ops), not as a standalone later `impl` (that trips the pre-existing
"impl is missing op" lint — unchanged behaviour).

9 tests added to `tests/ability_test.blsp` (runtime inherit/override/delegation, the four
checker interactions, cross-process): 54 → 63, all green. **Deferred (ADR-011):** `derive`
(Elixir `@derive` / Rust `#[derive]`) — auto-generate the *required* op structurally for a
record so `(defrecord point (x y) :derives [Ord])` needs no body; it composes directly with
provided ops (derive the one required op, inherit the rest) and is the natural next step.

## 2026-07-29 (cont.) — inference for multi-arity / variadic / optional closures

The inferencer bailed on any closure that wasn't single-arm-no-rest-no-optional — which is a
lot of std (every multi-arity or variadic function). It couldn't pin their *param* types
(those vary per arm), but it CAN infer the **return**: the union of each arm's tail with the
arm's binders bound to `ANY` (`infer_return_only`, a params-less `Sig`). That return flows to
callers; arity stays checked independently by `arity_of`, so a params-less sig loses nothing.

Sound because a union of arm returns is a *supertype* of whatever a given call actually
returns — it can only *under*-flag a caller, never false-positive; and any arm whose return
can't be typed defers the whole thing. A multi-arity `describe` returning `:one | :two` now
flags `(string-length (describe 5))`; a variadic `(joiner a & xs)` returning a string flags
`(+ 1 (joiner "x"))`; arity errors on the same functions still fire.

Cardinal-sin gate held over the bigger surface: type suite 270 → 273, zero new false positives
across std/ + tests/. Remaining inference lever: bottom-up fixpoint order so a caller of an
as-yet-un-inferred function resolves without depending on evaluation order.

## 2026-07-29 (cont.) — the per-process memory, finally attributed byte for byte

No code shipped in this stretch; it retired a standing "roughly half is unattributed" note
and killed three plausible-but-wrong ideas before any of them cost an implementation.

**The profile.** A temporary size-histogram in the counting allocator (an atomic per
allocation — measurement builds only, removed afterwards) gives the whole of `spawn` + park
at 300k processes: `Box<Process>` 1184 B, two 256s (one is `vm_call_ics`), two 160s (one is
`vm_fast_links`), `Arc<Mailbox>` 184, a 180, two 84s (one is `arm_ic_blocks`), `Suspended`
136, and ~340 across small buckets. It sums to **~3019 B against 3037 measured** — that is
all of it. Identities come from differencing the same workload under `(hibernate)`, which
drops exactly one 256, one 160 and one 84, so **the IC tables are ~500 B/process** by
measurement rather than by adding up `size_of`s. That makes M2b the biggest *reducible*
item, second only to the `Box<Process>` itself.

**Three corrections, all caught by testing the claim instead of building on it:**

- *"The fat IC table is cold after the fast-link collapse."* No — the collapse made it cold
  for the **JIT** path only. `vm_call_ic_probe` is the primary IC hit path in the VM
  dispatcher, so it is hot for every *interpreted* call. The `RwLock<HashMap>` design that
  followed from this would have regressed every un-JIT'd call site.
- *"mimalloc has a size-class cliff just above 1272, so the next win is binary — cut exactly
  184 B or get zero."* No. That was over-read from single-sample padding runs. Measuring the
  allocator directly (`crates/lisp/tests/size_class_probe.rs`, 200k live allocations per
  size) shows it is near-linear here: 1024 → 1039, 1208 → 1215, 1280 → 1295, 1536 → 1551.
  The reproducible +277 B/proc for +192 B of `Process` is ~1.44× page-level slack, not a
  class step. Incremental shaving keeps paying at roughly 1:1 — do not hold cuts back for a
  threshold that does not exist.
- *"Hang the shared IC slots on the `CompiledArm`, since ADR-175 already shares arms."*
  **Unsound**, and the failure mode is a wrong-callee miscompile rather than a stale-cache
  warning: `SHARED` is a process-wide `LazyLock` and every `Interp::new()` clones the same
  `Arc<SharedCode>` while building its **own** `RuntimeCode`, so a prelude arm is shared
  across *runtimes* — and an IC entry caches a *global* resolution, which is per-runtime.
  Shared IC state must live on `RuntimeCode`, keyed by `(arm_uid, site)`.

Also corrected: `(hibernate)` reclaims **12%** on a bare shell, not the ~40% recorded
earlier. That figure came from processes that had run enough code to populate their caches;
quote the workload with the number.

Kept as tooling: `size_class_probe.rs` (`#[ignore]`d — a measurement, not an assertion).
The allocator histogram was removed and its recipe written into `runtime-frontier.md`.

## 2026-07-29 (cont.) — `:derives`: per-ability record derivation (ADR-185 part 2)

The follow-on the provided-ops entry named as "the natural next step." A `defability` may
now declare a `:derive-record` recipe and a `defrecord` may `:derives [Ability …]` — Elixir
`@derive` / Rust `#[derive]`, but **each ability decides how it derives itself** (the recipe
maps a record's field names to `impl` method forms for the required ops; provided ops then
come free). `(defrecord point (x y) :derives [Columns])` synthesises `columns` and inherits
`ncols`.

**The decisive design point: derivation runs at LOAD, not expansion.** The obvious approach
— `defrecord` calls the recipe during macro-expansion and emits a static `(impl …)` — breaks
the checker, which macro-expands a file *without evaluating it*: the ability's recipe isn't
registered when a later `defrecord` expands, so the lookup returns nil, the expansion errors,
and it cascades to `unbound symbol`. (Confirmed empirically before choosing.) So `:derives`
expands to a `(derive-into 'A id 'fields (current-ns))` *call* run at load, where sequential
top-level eval guarantees the recipe is registered; `derive-into` evals each method form into
a fn and `register-impl`s it. A small checker pass reads the `derive-into` forms and marks
every op of the ability implemented for that id, so a derived record satisfies call-site and
`:sealed` checks without running the recipe.

Prelude only (`defrecord`/`defability` macros + `derive-into`) plus the one checker pass. 6
derive tests added (structural derivation, provided-op composition, the not-derivable error,
two checker interactions, cross-process); with the 9 provided-op tests, `tests/ability_test`
is 54 → 69, all green, `nest check` zero-warning.

> Process note: a `git merge` of `main` mid-work reverted the uncommitted prelude/checker/test
> edits for the provided-ops half (the committed devlog/roadmap entries survived); they were
> re-applied on top of the merge, so both halves now land together.

## 2026-07-29 (cont.) — the checker meets the REPL and LSP hover

The type checker ran in every batch path (`nest`, `brood`, MCP) and LSP *diagnostics*, but
two interactive surfaces were blind to it. Closed both, keeping the checker's own
soundness/advisory discipline.

**REPL advisory checking** (`std/tool/repl.blsp`). `repl--eval-print` now runs the checker on
each input before evaluating it (`mapcat check (read-all src)`, fragment mode). The REPL is
the one place *every def is loaded*, so inference applies to the whole live image — a call to
a just-defined function is checked against its *inferred* signature (`(string-length (dbl 5))`
warns right after `(defn dbl (x) (+ x 1))`). Fragment mode skips operand-unbound so a typo's
`unbound` isn't printed twice (eval raises that). Advisory: printed before the result, never
blocks it, silent when clean / on incomplete input / under `BROOD_NO_CHECK`.

**LSP hover type signatures.** Hover surfaced the arglist + docstring but not the checker's
*type* sig. Added `types::check::signature_string` (the tooling view of `sigs::sig_of`) →
`introspect::type_signature` → rendered under the arglist for a resolvable free name
(builtins/prelude/imports — stably loaded; a buffer-only def isn't loaded, so it's excluded
to avoid a stale sig). Hovering `map` now shows `(fn seqable -> seqable)`.

Both reuse existing checker entry points (no new checking logic), so they inherit the
zero-false-positive guarantee. LSP hover tests 8 → 9; REPL verified (clean/disabled/incomplete
all quiet).

## 2026-07-29 (cont.) — any ability name is a type; checker sees the live registries (ADR-186)

Two things, one fix. **The bug:** the checker read `ability/*abilities*`/`*sealed*`/`*impls*`,
but those globals are the **bare** earmuff-ambient `*abilities*`/`*sealed*`/`*impls*` (ADR-151
— an earmuffed name is never namespaced). So the reads resolved `unbound` and returned empty:
the checker had only ever seen a *file's own* `register-*` forms, never abilities/impls/sealed
reachable through `(:use …)`. Fixed to the bare names — surfaced only because the next feature
needed imported abilities visible. The missing-impl call check now sees imported impls too,
which only *removes* warnings (more impls found); zero new warnings across std/ + tests/.

**Open abilities as types (extends ADR-181).** A sealed ability resolved to the finite union
of its members; an *open* ability (no closed set) dropped the whole `sig`. Now every ability
name resolves — sealed → the union (unchanged), open → the permissive `any`. `any` is the
*sound* choice for open: impls are late/unbounded (`:default` may cover everything), so no arg
can be rejected on the type; the real safety is the missing-impl check at op call sites. The
payoff: `(sig render (Display -> string))` now survives (return + other params still checked)
instead of being discarded. `ability_type_table` (all abilities → sealed-members | open) +
`annot::ability_type` (→ union | `any`). Type suite 273 → 275; zero false positives.

## 2026-07-29 (cont.) — record patterns in `match` (ADR-187, part 1)

Closing the biggest structural gap from the "most-loved languages" review: first-class
matching on a record. A `defrecord` value is a map with a reserved `:__id__`, so it could
already be matched with `{:__id__ :geo/circle :r r}` — verbose, exposes the key, doesn't
assert record-ness. Added the concise `(record name {map-pattern}?)` pattern.

```lisp
(match shape
  ((record circle {:r r})        (* 3.14 r r))
  ((record rect   {:keys [w h]}) (* w h))
  (_                             :other))
```

**Keyword-field, not positional — and that's the right call, not a compromise.** Brood
records are hash-ordered maps (field order lives only in the `defrecord`), so, like
Elixir/Clojure, fields bind by key. Positional would also need the definition's field order
at macro-expand time — the same checker-fragility `:derives` hit (ADR-185) — so it's out.

Mechanics (all in the Brood matcher, `std/prelude.blsp`): a `record`-headed pattern (named
like `and`/`or`/`bytes`, since `(circle r)` is already a *list* pattern), id derived
**syntactically** via `ability--id-kw` (so it lowers identically in the checker's expand pass
and at runtime — no `*record-ids*` lookup), test is one `(%eq (record-id t) :id)` wrapping the
ordinary map-pattern compile (so `{:k p}`/`:keys`/`:or`/nesting compose free). No special
form. 10 tests added to `tests/pattern_matching_test.blsp` (incl. nesting + cross-process:
130 → 140), `nest check` clean.

**Next (ADR-187 part 2):** sealed-match exhaustiveness — warn when a `match` on a
sealed-ability-typed scrutinee (ADR-181/186) misses a member and has no catch-all.

## 2026-07-29 (cont.) — same-file function inference; the checker at the REPL + hover (ADR-188)

The last big inference gap: `sig_of` infers only *loaded* functions, so a file's own `(defn
…)`s were invisible while that file was checked — same-file callers got no result checking
(the whole point of `nest check`/LSP-on-a-file). `check_file` Pass 2.8 now infers each
function's return from its FORM (`infer_return_from_form`) and records it in `Ctx`, resolving
callees leaf-up over a bounded fixpoint: a function is stored only once its callees are final,
so nothing stale leaks (forward refs resolve; cycles defer). Return-only for now.

Enabling it earned its keep by forcing two soundness fixes (both latent, harmless only because
nothing consumed them before): a **reassigned global** was pinned to its `nil` init (Gap A
counted only top-level defs and read the value before the earmuff skip) → now defs are counted
recursively and earmuffed globals skipped, so a lazily-initialized `*g*` stays `dynamic`; and
**`%node-listen`'s primitive `Sig`** said `symbol` where a node name is a keyword (a real bug,
inconsistent with `%node-connect`) → corrected. Full std/ + tests/ sweep clean.

Also this stretch (earlier): the checker now runs at the **REPL** (advisory warnings before
each result, live-image inference) and **LSP hover** shows a name's type signature; and any
**ability name is a type** (open → `any`, sealed → member union), which required fixing the
checker's dead `ability/*…*` registry reads to the bare `*…*` globals.

Type suite 275 → 278; the four interactive/inference asks (REPL check, hover types, open
abilities as types, same-file inference) all landed with zero false positives.

## 2026-07-29 (cont.) — a float global silently bailed nbody's hottest arm (1.8×)

**`advance-body`, called 250 000 times in `nbody`, ran interpreted for the entire
benchmark.** `BROOD_DEOPT_TRACE=1` showed it deopting 16 times in a row and then going
`BAILED` — the deopt-feedback backstop doing exactly its job, on an arm that should
never have been deopting.

The cause is a gap in how float context is inferred. The tier-time profile
(`slot_tags`) snapshots the *live frame*, so it only ever types an arm's **parameters**;
let-binder slots read nil at that instant and get their types from the body's writes
during lowering. `has_float_slot` — the flag that lets `emit_prim2` route a type-erased
`Op::Handle` through the float path — is `slot_tags.contains(Tag::Float)`. But
`advance-body (b i)` takes a *vector* and an *int*. Every float it touches arrives from
`(nth bi k)` or from the global `dt`, so the arm reads as non-float-context, `(* dt nvx)`
falls through to the integer branch, and `as_int`'s tag-check deopts on the float — every
single activation.

Bisected on the source rather than guessed: replacing `dt` with the literal `0.01` took
the deopts to zero, and adding one *unused* float parameter did the same. Both confirm the
discriminator is the param profile, not anything about the arithmetic.

**Fix:** record which free globals an arm reads that held a `Value::Float` when it was
elected for tiering (`CompiledArm::float_globals`, filled by `record_float_globals` on the
thread that wins the election — the lowering thread has no `Heap`), and unbox those reads
to an `Op::Float` at the read site. Soundness is `as_f64`'s existing tag guard, not the
observation: a global that is no longer a float — a `def` since, or another runtime sharing
this arm's code — fails the guard and deopts to the VM. A stale guess costs a deopt and can
never miscompile, which is the same argument `has_float_slot`'s optimism already rests on.

`nbody` 0.36 → 0.20 s (**1.80×**), checksum unchanged, zero deopts.
`BROOD_NO_FLOAT_GLOBAL=1` is the off-switch / A-B lever.
Regression test: `tests/jit_float_global_test.blsp` (the nbody shape, a float global in a
comparison, a plain value round-trip, and a rebind-to-int after tiering).

**Worth remembering as a class, not a one-off:** a *perf* bug with no wrong answer and no
failing test, where the runtime's own self-healing (deopt feedback → `BAILED`) hid it by
converting a deopt storm into silent interpretation. The JIT-vs-no-JIT ratio is what
surfaced it — `fib` gets 54× and `collatz` 40× from the JIT, but `nbody` only 3.2×,
`bintree` 3.5× and `nqueens` 3.4×. That ratio per row is a cheap standing check.

## 2026-07-30 — sealed-match exhaustiveness (ADR-187 part 2)

Finished the second half of ADR-187: a `match` on a scrutinee typed as a sealed ability now
warns for any member no clause handles. Two sound pieces.

**The lattice fix (`annot::ability_type`).** A sealed ability's type was `Ty::union` over its
member record shapes — but `Ty::union` widens a differing `fields` map away, so it collapsed to
bare `map` and lost the member set (fine for rejecting non-maps, useless for exhaustiveness).
Built it instead at its true set-theoretic denotation: a single `%{__id__: (:a | :b | …)}` with
a keyword-lit union on `:__id__`. That's *equal as a set of values* to `⋃ₘ %{__id__: :m}`
because each member shape is an open record constraining only `:__id__` — so it's a sound
rewrite, not a widening. `Ty::union` stays untouched (field-wise-merging arbitrary records
there would invent cross terms). Bonus: `(sig f (Shape -> …))` now rejects a non-member record
precisely, not just a non-map.

**The pass (`check::exhaustive`).** Reads the un-expanded forms (a `match` is gone after
expansion), threading a `Ctx` — defn params seeded from their `sig`, `let` bindings — and at
each match resolves the scrutinee via `expr_ty`, extracts the `:__id__` lit set, checks
coverage. Sound by construction: unknown scrutinee / `:when` guard / any non-record,
non-catch-all arm → defer to silence; an unguarded `(record NAME …)` covers NAME (over-counting
a refutable inner only under-warns); ids compare by final `mod/NAME` segment.

**The one non-obvious bug:** ADR-188 made `register_declared_sig` qualify each sig target to the
file namespace, so inside a `(defmodule M …)` a defn's sig lives in `ctx.declared_sig` under
`M/name`, not bare — `sig_of` had to try the qualified key against `ctx` (not just the heap
store) or module code never resolved its scrutinee. Fixed; the walk now tracks the current ns.

7 tests in `tests/ability_test.blsp` (missing-member warns; exhaustive / catch-all / untyped /
guarded all silent; let threads the type; the message names the member). Whole-repo `nest
check` stays zero sealed-match false positives.

## 2026-07-30 (cont.) — per-file require-reachability lint closes KI-17 (ADR-189)

KI-17: `nest check` loaded the whole project image before checking, so a qualified reference
`mod/name` resolved for *every* file — even one that never `require`s `mod`. A file naming
`path/basename` without `(require 'path)` passed clean, then blew up at runtime the moment the
sibling that happened to load `path` first moved. Fixed by teaching the checker per-file
**reachability**.

**Mechanism / policy split.** `check-file{,-deps,-structured}` gained an optional reachability
set (module names the file may name qualified); `check_file_ext` unions it with the file's own
direct requires (`:use` / `:use-internals` / any nested `(require 'M)`) + its own ns and flags a
**user-written** `mod/name` whose `mod` is outside it. Only the whole-project driver sees every
header, so `std/tool/project.blsp` builds the module→direct-requires graph once (new native
`%module-direct-requires`), closes it **transitively** per file, and threads each file's set
through the fresh / cached / structured paths as **data** in the parallel `[file closure]`
chunks.

**Soundness, driven by the sweep.** Direct-requires-only lit 18 warnings on `std/`+`tests/`, 17
false. The transitive closure clears legitimately-transitive references (a test `(:use
editor/treesit)` naming `face/…`, since treesit requires `editor/face`); a `raw_qualified` guard
limits the lint to references the user *literally wrote* (never a macro-injected one); and it's
inert in single-file/LSP/REPL mode (an un-required module isn't loaded, so the ordinary unbound
check covers it). The lone genuine residue — `coverage` naming `project/…`, uncircle-able since
project requires coverage — got a *lazy* runtime `(require 'project)` in the one function that
uses it (idempotent, non-circular, the discipline the lint enforces). Net: **zero** false
positives across the whole tree; the lint fires precisely on the real bug (verified end-to-end
with a bad `nest check` project + a Rust regression test).

Two implementation notes: `collect_require_targets` needed the same `stacker::maybe_grow` guard
the rest of the checker uses (a pathologically deep form overflowed it); and the check-result
cache entry gained a 5th field (the closure), so a closure shift re-checks the dependent even
when its own mtime didn't move — cache version v1→v2. Docs: KI-17 → FIXED, ADR-189, `check-allow
:unrequired` category.

## 2026-07-30 (cont.) — correctness sweeps + a fuzzer-found KI-17 false positive

Two adversarial correctness sweeps after the KI-17 fix. **Kernel soundness:** the
recently-changed paths (single-copy local send, `defmulti` dispatch, IC rework) plus
maps/CHAMP and all 12 JIT tests, under `BROOD_GC_STRESS=1` + `BROOD_GC_VERIFY=1` +
`BROOD_JIT_VERIFY=1` on a debug-assertions release build — **clean, zero faults**.
**Checker false positives:** a battery of valid programs across every checker feature
shipped this session — clean.

Then a **generative reachability fuzzer** (hundreds of valid multi-module programs varying
import style — direct / `:use` / `:use-internals` / `:alias` / transitive — and nested
module names) found one real false positive: the KI-17 require-closure ignored the
`(:alias mod :as x)` clause, so a file that aliased a module and also named it qualified
(`mod/f`, or `x/f` which macro-expands to `mod/f`) was flagged as referencing an unrequired
module. `:alias` in fact `require`s its target, so `extract_import_module_names` now includes
it. Regression test `ki17_alias_clause_feeds_the_require_closure`; the fuzzer re-runs clean
(200 valid + 30 transitive → 0 false positives, 30 negative controls → 0 misses), and the
full `std/`+`tests/` sweep stays at zero.

## 2026-07-30 — occurrence typing: inferred params check callers (ADR-190)

Made the checker pay off on *unannotated* code. ADR-188 inferred each same-file function's
return but stored a **return-only** sig — deliberately, "so it never constrains an argument."
So `(needstr 5)` against `(defn needstr (s) (string-length s))` went unflagged even though `s`
is obviously `string`. Flipped that: Pass 2.8 now carries each function's inferred **parameter
demands**, and the caller arg-check consumes them.

**The soundness argument (why flagging callers from inferred params is safe).**
`collect_param_demands` under-constrains — only unconditional uses of known-sig callees
constrain a param — so the inferred param type is a *superset* of the true valid-argument type.
An arg disjoint from a superset is disjoint from the truth, so it genuinely errors; the flag is
never a false positive, only an under-warn. Whole-repo `nest check`: **zero** new warnings.

**Ability-op occurrence typing.** A use of a sealed op — `(area s)` — demands `s` be a member
(a non-member no-impls), so `s` infers to `%{__id__: (:circle | :rect)}` with no sig. Emitted
only when sound: the op is unambiguous (one ability), sealed (closed set), and `:default`-free.
`(shout 5)` → flagged; `(shout (circle 2))` → silent; add a `:default` → silent (any value ok).

**Two things I proved out and rejected along the way:**
- **Accessor occurrence typing is unsound.** `(point-x p) ⇒ p : point` (nominal) flags a bare
  `{:x 9}`, which works at runtime — a false positive `nest check` caught in `record_test`.
  Accessors are structural `get`; only dispatching ops carry sound nominal identity.
- The demand needs the sealed-op table built *before* Pass 2.8 (params are computed there), and
  a return-deferred function (e.g. one returning an ability op whose facts aren't on `ctx` yet)
  still needs its params stored — handled by a post-fixpoint pass.

Annotations stay optional and win when present. 7 tests in `sig_adoption_test.blsp`. Net: write
plain Brood, and a wrong caller is flagged — the derived-benefit goal.

## 2026-07-30 (cont.) — occurrence typing works cross-file (ADR-190 follow-up)

Closed the one gap in ADR-190: ability-op occurrence typing now fires **across files**, not
just within one. A `(defn shout (s) (area s))` in module A, called wrongly as `(shout 99)` in
module B, is flagged with `s : %{__id__: (:a | :b)}` — no annotation anywhere.

The bug was in `infer_sig` (the *loaded*-closure path `sig_of` uses for an imported function):
`let ret = expr_ty(tail)?` discarded the inferred **params** whenever the **return** couldn't
be inferred — and a function returning an ability op can't resolve its return on the bare
inference ctx (the ability facts aren't bound there). So it now returns the params with an
`ANY` return when a param is genuinely constrained — the loaded-path mirror of Pass 2.8's
post-fixpoint fallback. Sound (params under-constrain; the `ANY` return just defers).

Diagnosis note to self: the primitive-demand path (`label`) crossed files fine because
primitive sigs are global; the ability-op path didn't, which is what pointed at the
return-discards-params behaviour. (Also spent a minute chasing a stale `nest` binary — `nest`
is a separate binary embedding the lib, so `cargo build --bin brood` alone doesn't rebuild it.)
Whole brood repo stays warning-clean.

## 2026-07-30 (cont.) — the GC/VM/JIT correctness sweep: eleven defects, KI-18 and KI-19 closed

Three review passes over the GC, the bytecode VM and the JIT, then fixing everything they
found. Each fix carries a regression test that was **verified to fail without it**. Full
detail lives in the commits; this is the shape of what was wrong and the two lessons worth
keeping.

**The defects clustered around one wrong premise: "effects only live in calls."** Four
separate paths let a JIT deopt re-run a `table-put` — no journal for a call-free effectful
arm; a multi-arity self-call exemption that matched the head symbol and ignored argc (every
arm of a `defn` shares `dbg_name`); the leaf inliner splicing an effectful callee into an
engine that cannot journal; and `jit_run_fast_link` treating an IC re-probe miss as "let the
caller redo the call" *after* the callee's native code had already run. A 200 000-iteration
driver put 402 047 times. All four now count exactly.

**The rest, briefly.** A recursive `try` body SIGSEGV'd the OS process — `vm_apply` is the
one VM shape that consumes real Rust stack per Brood level (`try`/`&optional`/native
callbacks re-enter through it) and nothing on that path reaches `eval`, so the tree-walker's
byte guard never saw it; it now probes native headroom, the VM sibling of the JIT's KI-14
fix. The register worker hard-linked a non-tail self-call on `dbg_name` alone, so aliasing a
closure and rebinding the name kept the old body calling itself (`12` where every other
engine said `1001`). `Inst::SelfCall` never re-checked the global it loops on, so a
long-lived `(defn serve (state) … (serve next))` could never be hot-reloaded — the shape hot
reload exists for. Float `÷0` returned `inf` where the VM raises. LINMAP's wrapper
snapshotted a result its own linearity proof allows to be a non-table, and its rewrite
descended into `(quote …)`, corrupting data. Entry-hoisted globals raised `unbound symbol`
for branches that never execute. The RUNTIME generation free was neither single-flighted nor
epoch-validated — two processes could wipe a generation that had since become current. And
table ops were missing from the allocation predicate, so a hoisted pair-slab base survived a
`from_message` reallocation: a use-after-free alive only by allocator luck.

**Lesson 1 — the runtime's self-healing hid a bug for months.** nbody's hottest arm deopted
on every activation and the sixteen-deopt rule bailed it to the interpreter, so a deopt storm
became *silent interpretation*: no error, no failing test, just a slow row. The diagnostic
that exposed it is cheap and worth keeping as a standing check: **the JIT-vs-no-JIT ratio per
row.** `fib` gets 54× and `collatz` 40×; nbody was 3.2×, and `bintree` (3.5×) and `nqueens`
(3.4×) still sit in that band.

**Lesson 2 — one predicate should not gate two things with different widths.** Adding table
ops to `inst_may_allocate` fixed the use-after-free but also added a back-edge GC safepoint,
costing `sieve` 6%. The safepoint's justification ("the nursery grows unbounded") did not
survive checking: the back edge emits `brood_rt_tick_n` *independently*, so a native loop
already yields on its quantum. The gates are now deliberately asymmetric — a safepoint that
can fire must imply the hoist is off, never the reverse.

**Two attempts at KI-19 failed before the third worked, and the failures are the useful
part.** Staging the call head so the operator resolves first is correct but cost `json` 6×
(168 → 1159 ms) — because what the elided head really buys is the call IC's cached *arm*, not
the callee lookup. Doing it via a global-IC site fixed that but **aborted the process** on
every JIT'd row: `emit` decides elided-vs-staged from the callee node while `jit_lower`
decides from `(head, site)`, so a staged head left `head: None` with a live `site` and the JIT
resolved a head that wasn't there. The shipped form keeps `head`/`site` populated behind a
`staged` flag (IC still caches the arm, validated by closure identity) and exempts
**reserved** names, which `def` refuses under ADR-166 — without that exemption it cost `regex`
31%, almost all of it `first`/`rest`-class calls that were never rebindable.

**Measurement note.** Three apparent benchmark movements this session were drift, not results
— `spawn` +20%, `spawn-live` −22%, `persistent-map` +12.8% — all rejected by controlled `make
ab`. Two were reported as regressions before checking machine load. A single harness sample
does not separate signal from drift on this box **in either direction**; the −22% would have
been published as a win.

## 2026-07-30 (cont.) — occurrence typing: variadic false-positive fix (ADR-190)

Probing for gaps found one real **false positive**: a variadic `(defn vf (& xs) (fold + 0 xs))`
inferred `xs : seqable` (from `fold`) and the caller check applied it to *argument 1* — so a
valid `(vf 1 2 3)` was flagged "expects seqable, got 1". The rest binder collects the args into
a *list*, so its demand doesn't map to any single argument position. Fix: `infer_params_from_form`
now skips a fn with `&`/`&optional` (mirroring `infer_sig`, which already guards complex
closures on the loaded path — cross-file variadics were already clean). Battery of edge probes
(guarded, multi-arity, let-rebind, higher-order, structural accessor, member caller,
multi-demand intersection) is otherwise clean; whole repo stays warning-free.

## 2026-07-30 (cont.) — chained-guard narrowing (and/or) + inference-gap wrap-up

Closed the ADR-011-deferred guard-narrowing gaps in `check_if`. **`and`**: a truthy
`(and A B C)` proves every conjunct, so each narrows the then-branch — not just the first
(`and_conjunct_guards` descends the nested `(let (g cond) (if g rest g))` expansion). A falsy
`and` proves nothing, so the else-branch is left alone. **`or`**: when every disjunct is a
biconditional guard over one variable, the then-branch narrows to the union and the else to its
complement (`or_same_var_narrowing`); a then-only or cross-variable disjunct declines. All via
intersecting `narrow`, composing with the single-guard and path narrowings. Zero new warnings
across the `std/`+`tests/` sweep; 4 regression tests including negative controls (falsy-and,
different-var-or).

That, plus the parallel ADR-190 (inferred parameters check callers), leaves the inference arc
essentially done: control-flow returns, recursion, multi-arity/variadic returns, same-file
functions, inferred params, and/or narrowing, and HOF-callback results are all covered. The one
remaining piece — **per-arm parameter checking of a multi-arity callee** — stays deferred:
sound to leave (a missed check is a false negative, never a false positive), and closing it
needs an inferred-overload path + per-argc arm selection in the call-check for marginal value.

## 2026-07-30 (cont.) — super-abilities: `:requires` (ADR-193)

`(defability Ord :requires [Eq] …)` — an implementor of `Ord` must also implement `Eq`, enforced
at check time (Rust's `trait Ord: Eq`). Previously the dependency lived in a comment and surfaced
as a runtime `no-impl` deep in a provided-method body. `check_requires` (modelled on
`check_sealed`): for each id implementing an ability with `:requires`, every op of each required
ability must resolve (direct impl, `:default`, or provided op) — else *"ability Ord requires Eq:
:money implements Ord but has no impl of `eqv` for Eq"*.

Prelude clause + `*ability-requires*` registry (mirrors `*sealed*`) + one advisory checker pass —
no runtime change, no new special form. Composes with the rest: a required ability's provided op
is satisfied by its default; a `:default` covers any id; an unknown required ability defers (no
false positive). 5 tests; whole repo warning-clean (no std ability declares `:requires` yet).

This completes item #1 of the six deferred abstractions; the other five are recorded on the
roadmap with their urgency (all ADR-011 "wait for a concrete need" except open-ability bounds,
which is a declined non-goal).

## 2026-07-30 (cont.) — LSP goto-definition reaches macro-defined globals (defrecord/defability)

Reported symptom: `M-.` on a record constructor (`fib-job`) from another module found nothing.
Root cause was two-fold. (1) The cross-file def-site table is keyed by `def_form_name`, which only
recognizes a form whose *outermost* head is `def`/`defn`/`defmacro` — but `defrecord` expands to a
`(do (defn ctor …) (defn accessor …) …)`, so the `do` head matched nothing and the inner `defn`s
were never recorded. (2) The `load` builtin (the path the LSP uses to bootstrap project modules,
and reload) called `note_definition` **only on the un-expanded form**, unlike the file-runner in
`lib.rs`, which notes the expanded form too — so even a recognizable expansion wouldn't have been
seen under `(load …)`.

Fix, both halves: `note_definition` now **descends into a `(do …)`**, recording each inner
`def`/`defn`/`defmacro` at the outer call-site `pos`; and `load`/reload now note the **expanded**
form as well (matching the file-runner). Result: `(source-location 'foundry/fib-job)` /
`…/fib-job-n` / `…/run` all resolve to the `defrecord`/`defability` line, so cross-file *and*
same-file (via the `Free`→`source-location` fallback) goto work for constructors, accessors, and
ability op dispatchers — every macro-synthesized global, generally, not a per-macro special case.

Secondary gap closed in the same pass: the CST-based outline/workspace-symbols/hover layer
(`defs.rs`) only parsed `def`/`defn`/`defmacro`, so `defrecord`/`defability` never appeared in the
document outline. Added `DefKind::Record` (→ `STRUCT`, fields as constructor params) and
`DefKind::Ability` (→ `INTERFACE`). Left line 721's `eval-string`-class loop alone (no file
context, records no sites). 4 new Rust tests (2 in `defs.rs`, 1 end-to-end in `definition.rs`, plus
the empirical `source-location` probe); full `-p brood` + `-p brood-lsp` suites green; `docs/lsp.md`
step 1 updated (it had documented the "`do` isn't recorded yet" limitation explicitly).

## 2026-07-30 (cont.) — benchmark fairness, a published run, and three refuted perf hypotheses

The afternoon after the correctness sweep. Net: **`nbody` −47% published**, two benchmark
ports corrected, one memory win, and three hypotheses retired with measurements. Recording
the negatives at length because each one looked obviously right and cost real time.

**`spawn-live` was giving .NET, Node and Python unearned credit — two defects.** (1) .NET's
`TaskCompletionSource` resumes its continuation **inline on the setter's thread** by default;
probed at **1000 of 1000 inline, none on the pool**, so "wake 300 000 concurrent units" was
300 000 synchronous closure calls with no scheduling at all. `RunContinuationsAsynchronously`
fixes it. (2) Brood and Elixir have each unit *send a reply message* the parent receives
individually — two copied messages per unit — while Node/Python/.NET returned a value into a
pre-allocated array via `Promise.all`/`gather`/`WhenAll`, a reference store. All three now
drain a queue one item at a time. **Neither fix moved the standings**: forcing .NET to
schedule made it *faster* (345 → 294 ms, cores 1.0× → 1.6×). The credit is largely earned;
what those runtimes don't provide is structural, so the real fix was presentational — the row
now leads with Brood vs Elixir, its only peer.

**Payload representation.** Brood and Elixir copied a 16-cell cons list where the array ports
memcpy'd 64 bytes. Both now use their contiguous container (vector / BEAM tuple) — the mapping
`nbody` already documents, so both peers move together and the comparison stays level. Worth
**11% of wall and 0.6% of memory** under the harness. An earlier single-run pair had suggested
21%/33% and was published before being checked; the memory half was pure sampling noise.

**`sort` never had a memory defect.** Its "191 MB, 6× the field" was a *classification*
artifact: the row sat in the like-for-like table while pitting immutable-linked-list-sort
against in-place-array-sort. Memory splits exactly on that line — 124–191 MB for the three
persistent languages, 25–67 MB for the four in-place ones — and Brood is **1.19× Elixir** on
the same structure. Reclassified. Attribution, from `gc-stats` phase instrumentation: ~750 000
cons cells live at peak (the input list *and* the new sorted one, since immutability forbids
sorting in place) at 48 bytes each, doubled by the copying collector's to-space and again by
`Vec` capacity growth.

**Three refuted hypotheses — do not re-chase.**
1. *Nursery growth factor* (`2 × live`), called "the dominant term" for `sort`: sweeping
   2.0 → 1.5 → 1.25 → 1.1 moves it **not at all** (183–187 MB). `BROOD_GC_GROWTH` kept as a
   knob purely because it made the refutation possible; its doc says so.
2. *Tenure-path nursery reservation*: this one is real but small — the nursery restarts empty
   on a tenure yet reserved the outgoing nursery's full length. Worth −7.7% json, −7.5%
   base64, −3.9% bintree, time flat. Shipped.
3. *Installing the callee's IC block on a fast link* (KI-20): correct, and **reverted** at
   `bintree` +5.5% for no gain — the bases lookup lands on `jit_dispatch_fast_frame`, the path
   whose whole purpose is to skip the IC probe, and which is dominated by self-recursion where
   the install is a no-op. Any retry must pass the bases through the IR alongside the
   `code`/`nslots`/`env` it already loads.

**A diagnostic worth keeping: the JIT-vs-no-JIT ratio per row.** `fib` 38×, `loop` 31×,
`collatz` 28× at the top; `reduce`/`strings` 1.0×, the codec rows 1.1–1.2×, `sort` 1.3×,
`pipeline` 1.3×. It is what exposed nbody's silent bail, and it cheaply separates
"interpreter-bound" from "JIT working". Caveat learned the same day: `jit_native` counts
native *entries*, so a self-tail loop shows **1** for a whole 375k-iteration run — `sort`'s
`jit_native = 2` is correct, not a bail.

**Measurement discipline.** Three apparent benchmark movements this session were drift and
were rejected by controlled `make ab`: `spawn` +20%, `spawn-live` −22%, `persistent-map`
+12.8%. Two were reported as regressions before machine load was checked; the −22% would have
been published as a win. A single harness sample does not separate signal from drift on this
box **in either direction**. Also fixed a self-inflicted version of the same problem: nine
orphaned wait-loops (`until ! pgrep -f <pattern>`, where the pattern matched the watcher's own
command line, so the condition could never go false — oldest ran 17 hours) were polluting the
liveness checks used to decide whether the machine was quiet.

## 2026-07-30 (cont.) — LSP + MCP: records/abilities become first-class to the tooling

A follow-on sweep after the goto-definition fix, closing the same macro-generated-global blind
spot across the rest of the tooling. Two survey agents (one LSP, one MCP) found the gaps; four
landed.

**LSP correctness.** `scope.rs`'s `collect_globals` registered document globals only for
`def`/`defn`/`defmacro` — so a record constructor or ability name defined *in the buffer*
resolved `Free`, and hover / signature-help / same-file-goto (all gated on `Defined{Global}`)
showed nothing. Added `defrecord`/`defability`/`defdyn` to it; one change fixes all three
features. Then the **P0**: renaming a `defrecord` rewrote the record name and its constructor
calls but **not** the `foo-<field>` accessors the macro synthesizes — after the record
re-expanded as `bar-<field>`, every `foo-<field>` call site dangled. `workspace.rs::rename` now
cascades: it finds the record's `(defrecord … (fields))` form (in a file that resolves the name
back to the target, so two same-named records don't cross wires) and renames each accessor in
lockstep. An ability needs no cascade — its op names are independent of the ability name.

**A `type-signature` builtin.** `(type-signature 'name)` exposes the checker's arrow signature
(`crate::types::check::signature_string`) to the language — the same string the LSP hover shows.
Thin Rust-over-the-checker bridge; both LSP and MCP now share one source for "what's this name's
type."

**MCP.** The ability system was invisible over MCP. Added `abilities` (list) and `ability`
(describe one: ops with provided-default flags, sealed members, `:requires`, owner, derivable,
and implementors computed from `*impls*`), `check-source` (type-check a snippet string via
`check-string-structured`), and folded `:type` into `lookup`. Also: `check`/`run-tests` now use
the structured `mcp--error-shape` like every other tool, and `wrap_as_mcp_content` sets MCP's
`isError` on any soft-error result.

**Semantic tokens + keyword classification.** `defability`/`impl` were missing from
`SPECIAL_FORMS` (the shared source of truth for highlighting/completion/grammar), so they
coloured as ordinary calls; added them. A `defrecord` name now tokenizes as `STRUCT` and a
`defability` name as `INTERFACE` (legend + role classification), instead of both reading as
plain functions.

Consciously deferred (genuine nice-to-haves the survey rated low): record-field completion
inside a constructor/map, and `impl` op-body snippet insertion (both depend on fiddly cursor/paren
context — a mis-fire would malform an insertion). Doc drift fixed: stale `mcp.rs` "step 3" /
"prompts empty" comments, the `brood://project` promise (dropped — reachable via `eval`), and the
`docs/mcp.md` / `mcp.blsp` tool counts (17/20 → 23). Tests: 6 new LSP (rename cascade, in-buffer
record/ability goto, STRUCT/INTERFACE tokens, defrecord/defability outline), 4 `type-signature`,
6 MCP-tool. Full suites green: 450 lib, 126 brood-lsp, 3914 in-language.

## 2026-07-30 (cont.) — LSP: `impl` op-body snippet completion

Follow-up to the records/abilities tooling sweep. Completing an ability op inside `(impl …)`
now inserts a fillable method skeleton — `(area [self] $0)` — instead of just the bare op name,
so you fill the body rather than retype the shape. `impl_method_skeleton` builds it from the op's
arity (first param `self`, then `arg2`…), and detects whether the method's own `(` is already
typed (cursor inside `(op…` → omit the wrapping parens) vs sitting directly in the impl form
(supply them). Snippet syntax (`${1:self}`/`$0`) is **gated on the client's `snippetSupport`**:
the server now reads that one capability at `initialize` (previously it discarded the client
params entirely) and threads a `snippet_support` bool through `main_loop`/`handle_request` to
completion; a non-snippet client gets a plain `(area [self] )` skeleton, never literal `$`.
2 tests (the skeleton's four paren/snippet modes + the end-to-end insert_text). The sibling
type-directed record-field completion is on the roadmap backlog — it needs inferred types in the
completion path, worth ~a day, deferred until then.

## 2026-07-30 (cont.) — supervised `spawn-link` was quadratic: the supervisor's child list → a pid-keyed map

Started from "get `spawn-link` to Elixir, we are way behind". **The primitive
itself is not behind** — measured four ways against `elixir -pa` precompiled
modules (checksums verified; `.exs` scripts pay ~100 ms of module compilation per
run and must not be used for this):

| N = 100 000, compute (wall − startup) | Brood | Elixir |
|---|---|---|
| `spawn` fan-out + fib(15) + reply | 238 ms | 212 ms |
| same with `spawn-link` / `spawn_link` | 274 ms | 271 ms |
| **marginal cost of the link itself** | **0.36 µs** | **0.59 µs** |

At the published `spawn` size (N=10 000) Brood is 39 vs 23 ms, reproducing the
benchmark row; linking costs us *less* than the BEAM. So the gap the row hints at
is process creation, not linking.

**The real gap is the supervised path**, which is what `spawn-link` exists for.
`nest`-shaped dynamic supervisor, N children added one at a time:

| N | old | Elixir `DynamicSupervisor` | ratio |
|---|---|---|---|
| 1 000 | 129 ms | 15 ms | 8.6× |
| 4 000 | 998 ms | 28 ms | 36× |
| 8 000 | 3 579 ms | 42 ms | **85×** |

Per-child: 110 → 447 µs as N grows — linear per op, i.e. **quadratic total**,
against Elixir's flat ~5 µs. `supervisor--do-start-child` ended with
`(append (get state :children) (list child))`: an O(N) list copy per child.
`restart-child` was worse — `find` + `map`-replace over the same list reached
**19.5 ms/op** at 4 000 children.

**Fix (in Brood, `std/proc/supervisor.blsp`):** `:children` is now a **map pid →
child record** instead of a list. Start order — needed by `which-children`,
`:rest-for-one` and shutdown ordering — lives on each record as `:seq`, stamped
from a monotonic `:next-seq`, and the ordered views sort by it (they are O(n)
operations anyway). `:ids` indexes `:id` → pid so the by-`:id` client API stays
direct too. The list-era helpers (`supervisor--replace`/`--drop`) are gone,
replaced by state-level `--put`/`--remove`/`--swap`/`--rebuild` so the
children/ids invariant lives in one place.

Measured on **one binary** (the pre-change module `load`ed against the same
build, so no profile/feature drift):

| | old | new |
|---|---|---|
| `start-child`, N=8 000 | 3 566 ms | 572 ms (**6.2×**) |
| per-child at N=8 000 | 442 µs | 65 µs |
| `restart-child`, K=1 000 | 963 µs/op | 100 µs/op |
| `restart-child`, K=4 000 | 20 873 µs/op | 123 µs/op (**169×**, now flat in K) |

20/20 supervisor tests pass, plus `link`/`gen`/`agent`.

**What is left, and why it is a kernel item, not a supervisor one.** Isolated
per-variant runs (each in its own process, best-of-5, N=4 000) attribute the
remaining ~65 µs per `start-child`:

| variant | µs/op |
|---|---|
| round trip + `spawn-link` + `link`, no bookkeeping | 16.75 |
| + map bookkeeping, pid only | 18.75 |
| + retain the record without its `:start` closure | 22.75 |
| + retain the full record (`:start` closure included) | **64.75** |

So retaining the child's `:start` closure costs ~42 µs. The supervisor's own GC
stats (read out of the supervisor process) say why: **324 collections copying
2.69 M objects, 189 ms of pause** out of 278 ms total, against 35/68 k without
the closure. Tenuring is working; each retained closure is simply ~670 objects,
because **`send` deep-copies a closure's code**:

| a `(fn () (spawn-link (idle)))` thunk | retained footprint |
|---|---|
| built and held in-process | 48 bytes |
| the same thunk received via `send` | **436 bytes** (9×) |
| received and discarded | 36 bytes |

`closure_to_message` (`process/message.rs`) deep-copies every arm's body forms
per message, and `closure_from_message` re-allocates them in the receiver. For a
**same-runtime** send that is avoidable: all processes of a runtime already share
one code region, and `spawn` already `promote`s its thunk into it. Sharing the
code on a local closure send (copying only the captured locals, as now) would cut
the retained record ~9× and take `start-child` from ~65 µs toward ~23 µs — and it
is general, paying off for every closure that crosses a process (child specs,
`gen` callbacks, task fan-out, `offload`), not just supervisors. Cross-*node*
sends must keep serializing the code (different runtime). **Done in the next
entry** — though not in the form proposed here: promoting a local closure on send
leaks the shared region, so only the *already-shared* case ships (ADR-194).

## 2026-07-30 (cont.) — closure sends share already-shared code; promoting-on-send measured and rejected (ADR-194)

The follow-up to the supervisor entry above, and it did not survive contact with
measurement in its proposed form.

**Where the sends actually go.** `BROOD_L1_STATS=1` on the supervisor bench: of
8002 local sends, 4002 hit the L1 fast path, **3996 were parked-but-declined**,
and only **4** were not-parked. `copy_cross_heap` declines closures outright, so
a closure-carrying message loses the fast path *for the whole message* — the spec
map included. That put the fix in the L1 copier, with both heaps in hand, and
meant `to_message`/`from_message` and the cross-node path never had to change.

**Attempt 1 — promote any sent closure (rejected).** Implemented first, because
it also covers inline capturing thunks. It works and is fast, and it leaks: a
promoted closure lands in the append-only RUNTIME region, so a *transient* one
(sent, used, dropped) needs a full aging/drain/free cycle to reclaim instead of
dying at the next minor GC. Peak RSS over N sent-and-discarded closures:

| N sent | promote-on-send | copy (baseline) |
|---|---|---|
| 100 000 | 129 MB | 112 MB |
| 200 000 | 190 MB | 121 MB |
| 400 000 | 340 MB | 143 MB |
| 800 000 | **541 MB** | 180 MB |

Growth proportional to closures sent — a leak in any long-running receiver, which
is precisely what this runtime is for. `BROOD_RT_GC_FLOOR=64` (aging as hard as it
goes) barely moved it: the blocker is reclamation convergence (ADR-091 stage 4),
not the handoff.

**Shipped — share only what is already shared.** A closure whose value is already
a RUNTIME-region handle crosses by handle; nothing is ever promoted on send. No
new region entries, so no growth: the same 800k run is flat at 150 MB.

The rule lands where it matters because **a closure that captures no locals is
already a RUNTIME value**. Measured per send: `(fn () (spawn-link (idle)))` — only
globals — **6 µs**; `(fn () (+ i 1))` capturing a local — **54 µs** (declines to
the copy, as before). So the idiomatic child spec is covered and a capturing one
still works, just at the old price. Guards: `region() == RUNTIME`,
`shares_runtime_with` (the process REGISTRY is global — a second `Interp` in one OS
process has a different region), and `BROOD_NO_SHARE_FN=1` as the off-switch.

**Result on the supervised path** (N=8000 `start-child`, wall incl. ~18 ms startup):

| | wall |
|---|---|
| list-based supervisor + copied closures (start of day) | 3 882 ms |
| pid-map supervisor, `BROOD_NO_SHARE_FN=1` | 575 ms |
| pid-map supervisor + shared closures | **251 ms** |
| Elixir `DynamicSupervisor` | 228 ms |

**16× end to end, and 5.3× → 1.1× of Elixir on wall** (compute 233 vs 44 ms, so
~5× on compute). L1 hit rate on that path: 50% → 100%.

**Stability, which was the gating requirement.** Lock order first: the copier runs
holding the receiver's mailbox mutex, and the only writer of `promote_lock` is
`age_runtime`, which holds it across an atomic generation flip and touches no
mailbox, no allocation, no I/O — so it cannot invert. Soundness of a retained
shared handle: the drain's Phase 2 walks the receiver's whole LOCAL heap, so
`runtime_gen_referenced` sees it and won't free underneath; aging never moves
handles and compaction requires unique ownership. Then measured, all on the
debug-assertions build (per-deref GC tripwire + heap verifier armed), each config
paired against `BROOD_NO_SHARE_FN=1` and required to produce **identical
checksums**: mass spawn + retained shared handles + hot reload (`def` rebinding
under load) at `BROOD_RT_GC_FLOOR=64`, the same under `BROOD_GC_VERIFY=1`, and
under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`. Five distribution-chaos runs
(3 × `dist_chaos_remote_spawn.sh` — closures over the wire — and 2 × `dist_chaos.sh`)
with `crashed=0` and no panics. Zero tripwire or verifier reports anywhere.

## 2026-07-30 (cont.) — a benchmark row for what a server actually feels like

The suite could not see the thing this runtime is *for*. Every concurrency row measured
throughput with a **closed loop** — the generator waits for each reply before sending the
next — which is coordinated omission: when the system stalls, the generator stops sending
and the stall never enters the numbers. So a runtime could win every row and still be the
one that feels bad in production, and nothing here would say so.

The new `latency` row (brood-benchmarks `dc7a35f`) is open loop: request *i* is scheduled
at `start + i × (1s/20,000)` regardless of whether the system keeps up, and latency is
measured from that scheduled instant, so queueing delay lands in the number. Every 20th
request occupies ~500 µs of CPU; percentiles cover the **other 95%**, so the question is
what a busy handler does to everyone else. Offered load is 0.5 cores of twelve — nothing
is capacity-limited, and the tail is scheduling rather than saturation.

| | p50 | p99 | p99.9 | max | cores | CPU·s |
|---|---|---|---|---|---|---|
| Elixir | 8 µs | 59 µs | 98 µs | 601 µs | 1.9× | 5.28 |
| **Brood** | 121 µs | 439 µs | 1300 µs | 2134 µs | 1.3× | 3.32 |
| Node | <1 µs | 451 µs | 561 µs | 1047 µs | 1.0× | 2.55 |
| Python | 42 µs | 478 µs | 624 µs | 852 µs | 1.0× | 2.53 |
| .NET | 4 µs | 714 µs | 12627 µs | 15082 µs | 2.4× | 6.04 |

.NET has the best median in the field and the worst tail by 20× — a 3157× spread from p50
to p99.9 against Elixir's 12×, while spending the most CPU of any port. Node and Python,
single-threaded, beat it at the tail: their p99 is exactly one fat request's duration,
because you wait behind at most one and then it stops.

**Three methodology mistakes, each caught by measuring rather than reasoning**, and all
three are the kind that would have produced a publishable-looking but meaningless row:

1. **Closed loop** (above) — fixed before the first numbers.
2. **Sizing the fat request in work units.** Units take 20× longer on a runtime 20× slower
   at arithmetic, so the row would have re-measured compute speed — which every other row
   already does. Now each port calibrates its own loop to ~500 µs at startup *and reports
   what it achieved*, so a mis-calibration is visible instead of silently voiding the
   comparison. That check paid immediately: calibrating **cold on a JIT** sized Brood's fat
   request 25× too small (2361 units, warm cost ~20 µs, against a 500 µs target), because
   the single sample measured the interpreter. Warm first, then take the best of nine.
3. **Percentiles over all requests.** Fat requests are 5% and take ≥500 µs by construction,
   so they occupied every high percentile and hid the only interesting question.

Occupancy is real work rather than a clock spin for two reasons: it allocates, so the GC
participates as a handler's would; and spinning on many .NET pool threads **crashed the
CLR** — `Internal CLR error (0x80131506)` in ~3 runs of 10, reproduced with both
`Task.Run` and `ThreadPool.UnsafeQueueUserWorkItem`. With calibrated work, 0 failures in 10.

**What it says about Brood**, recorded as `runtime-frontier.md` A4 rather than buried: our
121 µs median is 15× Elixir's and is per-message cost (A1/A3 again — a request here is
spawn + send + collector receive); our 1300 µs p99.9 is 13× Elixir's and is *scheduling* —
a fat handler should cost its neighbours nothing at 0.5 cores of load on twelve, and it
costs them milliseconds. First hypothesis to test is spawn placement: processes are placed
at spawn and never migrated, so a dispatcher spawning every handler can pile them onto its
own worker, where one 500 µs handler blocks the queue behind it. We also used 1.3× cores
where Elixir used 1.9× on identical offered load, which fits. Not yet investigated.

Node, single-threaded, has a better p99.9 than we do. That is the honest headline.

## 2026-07-30 (cont.) — KI-20 fixed: JIT fast link installs the callee's IC block

The last open known-issue. `jit_run_fast_link` (the shared body of the in-IR fast-link and
`jit_dispatch_call`'s fast-link caller) set the callee's env/dbg-fn/native-depth/stack-limit
before entering its native code but **not** `ic_bases` — so the callee ran against the
*caller's* per-arm inline-cache block. Never a wrong answer (every IC read re-validates
`sym`/`argc`/`epoch`, so a crossed entry just misses) but both arms ran permanently
cache-cold, and `dbg_site_loc`/`[jit-staged-stale]` reported the wrong site. The cloning
native-link path already installed the callee bases; only the fast path didn't.

Fixed exactly the way the reverted 2026-07-30 attempt's post-mortem prescribed — **no
hot-path lookup**. The callee's bases now ride in the `FastLink` slot (`_pad` → `callee_ic_base`
+ a new `callee_gic_base`); `vm_call_ic_fast_link` stamps them from the entry's already-resolved
`CallIcEntry::callee_bases` (so no `vm_arm_block` call and no borrow on the publish path, and the
memoised hot path returns them straight from the slot it already read); the IR loads them
alongside `code`/`nslots`/`env` (two `u32` loads from the same cache line) and passes them as two
more args to `brood_rt_fast_frame`; and `jit_run_fast_link` does `set_ic_bases`/restore around the
native call — two `Cell` writes off handed-in values, the runtime never re-reads the table. A
native flat cell carries `(0,0)`. The first attempt regressed `bintree` +5.5% because it read the
bases via a `RefCell` borrow + bounds-checked index *per call* inside `jit_dispatch_fast_frame`;
this one adds none, and a pinned best-of-9 A/B (`fib`, `bintree`) measured **+0.0%** vs a +0.0%
base-vs-base floor.

A debug cross-check in `jit_dispatch_fast_frame` now asserts the IR-passed bases equal the
authoritative IC's (`b == callee_bases`), tripping on any future mirror desync across the whole
debug suite. Verified: jit (35) + differential + jit_runtime_compaction green under
`BROOD_GC_STRESS=1 BROOD_JIT_VERIFY=1`, full `make test` green. `docs/known-issues.md` now has
**no open issues**.

## 2026-07-30 (cont.) — the tail was spawn placement: one threshold, p99.9 5× better

Acting on the three gaps the `latency` row exposed. Two of the three are now fixed, and the
first one turned out to be a one-line policy question rather than anything structural.

**The diagnostic came first, and it was decisive.** Running the latency workload with
`(sched-stats)`: 12 workers, 20 002 spawns, **2406 steals (12%), 0 migrations**. So 88% of
handlers ran on whichever worker the dispatcher happened to be on. `pick_spawn_worker` placed
every child on the spawner's own worker (BEAM model, cache locality, no scan) and relied on
work-stealing to rebalance — and stealing, which only takes *not-yet-started* work, got to
one in eight.

**First attempt: force round-robin. Wrong answer, and the reason is the interesting part.**
`BROOD_SPAWN_RR=1` moved `latency` p50 141 → 12 µs and p99 674 → 168 µs — and cost
`supervisor` **2.6×** (862 → 2223 ms). A supervisor's `start-child` is a request/reply that
spawns one child; scattering each child to a different worker turns every one into a
cross-worker wakeup. Two shapes, opposite preferences.

Also worth recording as a measurement lesson: my first attempt to test the placement
hypothesis in *Brood* (split dispatch across K dispatcher processes) produced nonsense —
1/2/4/8 dispatchers gave 2661/486/5423/5929 µs at p99, non-monotonic — because each
dispatcher **spins** on the arrival schedule, so K dispatchers permanently burn K of 12
workers. The experiment measured its own instrument. The env-knob A/B in the kernel was the
right test.

**Shipped: spill on backlog.** Keep the child local while our own queue is *empty* — the case
the locality argument is actually about — and spill round-robin once anything is waiting.
`BROOD_SPAWN_SPILL`, default 1. Cost is one `try_lock` on our own queue (a failed try_lock
reads as "no backlog", which is right: the only contender is a thief, and a thief means the
queue is draining anyway).

| `latency`, median of 5 | p50 | p99 | p99.9 |
|---|---|---|---|
| always-local (before) | 141 µs | 674 µs | 2902 µs |
| **spill ≥1 (now)** | **27 µs** | **232 µs** | **562 µs** |
| always round-robin | 12 µs | 168 µs | 3864 µs |

Swept 1/2/4/8: monotonic in p50 and p99, so 1 it is. Note the shipped setting beats *both*
extremes at p99.9.

**Also shipped: the selective-receive scan no longer takes the mailbox mutex per candidate.**
The tag pre-filter needs only the envelope's tag, but the scan released and re-acquired the
lock for every rejected message — a lock round-trip per queued message on every receive, paid
by exactly the processes that can least afford it. Skipping all rejected candidates under one
hold: per ref-pinned round trip against a backlog, 500 → 16/6 µs, 2000 → 48/13 µs, 8000 →
176/44 µs (before/after), and unchanged at 3 µs with no backlog. Still O(backlog); the
receive-mark is what makes it O(1), and is still open.

**Validation.** Full suite green. Unpinned A/B against `af25b7b3`: `spawn` −10%,
`pingpong`/`ring`/`pfib`/`supervisor` flat. `spawn-live` first read +8.9%, which a
base-vs-base control demolished — **the same binary against itself spread 20.6%** (2383 vs
2873 ms), and the new build then measured −9.4%. That row cannot resolve anything under ~20%
on this box; this is the fifth time it has produced a phantom. The `make ab` sweep was also
the wrong instrument here and I nearly published it: it pins to **one core**, which is why
`spawn-live` reads 6740 ms there against 2469 in the harness — and a single-core pin makes a
*placement* change meaningless by construction.

What remains of the latency gap is the **median**: 27 µs against Elixir's 8 µs, which is
per-message cost (a request is spawn + send + collector receive), not scheduling. Recorded as
`runtime-frontier.md` A5/A6.

## 2026-07-30 (cont.) — the receive-mark (ADR-195), and a correction to this morning's p99.9 claim

**The receive-mark shipped.** Every synchronous call in Brood is `(let (r (ref)) (send …)
(receive ([:reply ^r v] …)))`, and each one scanned the mailbox from the front — so a busy
process paid for its own backlog on every reply, which is precisely backwards. Envelopes now
carry a monotonic arrival sequence; `(ref)` stamps the sequence current at the moment it mints
(one relaxed atomic load off a lock-free `seq_hint`, no mailbox lock); and a `receive` whose
clauses *all* pin that ref binary-searches to the first message that could carry it. Sound
because a message enqueued before the ref existed cannot contain it — refs are unforgeable
counter values. Every uncertainty (clauses disagree, pin is not a ref, ref not the one we last
minted) declines the hint and scans from the front.

Per ref-pinned round trip against a tag-rejected backlog, this morning → now: 500 → 16/4 µs,
2 000 → 50/4 µs, 8 000 → 175/4 µs, **32 000 → 653/4 µs**. Flat: O(backlog) → O(1), 163× at
32k, and unchanged at 3 µs with no backlog. Validated on the armed build (GC tripwire +
verifier), five distribution-chaos runs, and six new tests pinning the cases where it must
*not* skip — a foreign ref whose reply predates our own mint, a second ref evicting the mark,
the same ref serving two receives, and a message queued before the ref still being there
afterwards.

**Correction: this morning's "p99.9 5× better" was a measurement error.** The spawn-placement
entry quoted the `latency` row as 2902 → 562 µs at p99.9. That number was not a median — the
sweep sorted its five runs **by p99** and printed the middle run's whole p50/p99/p99.9 triple,
so the p99.9 shown was whichever value that one run happened to score. Measured properly, with
per-metric medians over 11 runs, two samples of the *same* binaries disagreed on p99.9 by 3×
(baseline 3574 vs 3028 µs; new build 1139 vs 3484 µs). **p99.9 is not resolvable on this
workload at this sample size**, and the docs now say so instead of claiming a win.

What survives is solid and agrees across every measurement taken: **p50 136 → 27 µs (5.0×)**
and **p99 735 → 256 µs (2.9×)**, with the threshold sweep monotonic in both.

The lesson is the same one this repo keeps relearning, in a new costume: the statistic has to
be computed the way it is reported. "Median of five" was true of the *run*, not of the *metric*,
and the difference was invisible until a metric with 3× variance sat in the same row as two
stable ones.

## 2026-07-30 (cont.) — soak: 1.5M self-checking iterations, and RSS is not what it looks like

Answering "are we sure we gained this without losing stability?" with evidence rather than
assurance. A soak is sustained load with an invariant checked on *every* iteration — the
distinction matters here, because the failure mode I was worried about (a wrongly-applied
receive-mark) does not crash: it silently fails to deliver, and a survive-only soak would
sail straight past it.

Each iteration: a ref-pinned round trip against a **64-message backlog** (the reply must
arrive *and* all 64 junk messages must still be queued afterwards — the direct detector for a
wrong skip), a supervised child crashed and restarted, a shared closure and a capturing one
crossing a send, a hot-reloaded global, and a 32-process spawn burst. The detector was itself
verified by deliberately corrupting an invariant: it printed `ERROR at iteration 0` and halted.

Two 30-minute runs, the second a **control with every one of the day's mechanisms reverted**
via its off-switch (`BROOD_NO_RECV_MARK=1 BROOD_NO_SHARE_FN=1 BROOD_SPAWN_SPILL=999999`):

| | iterations | RSS | errors |
|---|---|---|---|
| all new | **792 829** | 860 MB | **0** |
| control | 747 818 | 2069 MB | **0** |

~26 M spawns and ~52 M messages per run. The new code did **6% more work on 2.4× less
memory**, and the gap widened monotonically through the run — the strongest evidence yet that
the day's changes help a *sustained* workload rather than only a benchmark.

**Right-sizing the run was itself a judgement worth recording.** The first attempt was two
3-hour runs; that is the wrong instrument. Correctness here scales with *iterations*, not
wall-clock, and 30 min already buys ~800k iterations (~52 M messages). Three hours buys 6×
that — and a bug needing 30 M iterations to surface would not be reliably caught at 30 M
either. That is an overnight, unattended job, not one to sit and watch.

**What the soak actually found** was not the thing it was hunting: RSS climbs steadily and
never plateaus (276 → 600 MB over 500k iterations), in *both* configurations. Two probes
turned an alarming curve into a result — removing hot reload changed nothing (so not the
RUNTIME region), and after 77k iterations with **2.5 M spawns** the live set was **4 processes
and 59 KB** against 207 MB RSS, falling only to 178 MB after quiescing and a forced
collection. So the language's accounting is clean and the pages are held by the allocator.
Recorded as `runtime-frontier.md` **A8**, with the consequence stated plainly: **RSS is not a
proxy for live data on this runtime**, and the retained figure tracks cumulative churn rather
than a working-set peak, which points at fragmentation rather than a high-water mark.

## 2026-07-30 (cont.) — chasing the leak that wasn't, and two corrections

Went after the open items with "tackle all potential issues" as the brief. The headline is
that **the leak does not exist**, and that finding cost two wrong entries before it was
measured properly.

**A8 was wrong twice, both times from the same mistake.** Version 1 blamed message/spawn
churn; version 2 blamed hot reload at ~75 KB per `def` (and quoted "~270 MB/hour"). Both were
artifacts of differencing **time-boxed** runs: a time-boxed run does a different number of
iterations per configuration, and RSS tracks iterations — so every such comparison measured
the iteration count rather than the thing under test. Measured with a **fixed iteration
count**, repeated, medians:

| 40 000 iterations | RSS delta |
|---|---|
| 0 reloads | 94.1 MB |
| 1 000 reloads | 91.7 MB |

Hot reload costs nothing measurable. A `def` in isolation is ~500 B and does not scale with
live-process count (0/200/2000 processes → 518/489/452 B). `:runtime-closures` never moved in
any workload (68 before and after, threshold 4096), so the shared code region was never
implicated and there is no ADR-091 reclamation problem visible here. What grows is churn —
20k→51 MB, 160k→284 MB, sublinear but no plateau — against a live set of 59 KB after 2.5 M
spawns. That is allocator fragmentation, and `MIMALLOC_PURGE_DELAY=0` recovers 17% here (2.3×
on a heavier-churn workload) for ~4% throughput. A8 now carries the lesson in the entry:
never difference time-boxed runs.

**The receive-mark's cost, measured properly.** The published run showed `pingpong` +6.5% and
`ring` +9.6%. Two A/B attempts disagreed with each other (+5.7%/+4.0%, then +4.5%/+4.9%), and
one configuration that should have been *slower* came out faster — the tell that it was drift.
A base-vs-base control put the floor at **0.5%** on those rows, against which the honest
figures are **pingpong +2.8%, ring +2.3%**. Real, and now published as such. I tried to
recover it by moving the per-send atomic store onto `(ref)` (sends outnumber ref creation, and
those rows never call `ref`); the store was not the cost, so the change stays only because it
is the better shape, and the residual is structural — a `seq` field per envelope and an
argument per receive.

**Hot reload does not reach a self-recursive loop.** A tail self-call compiles to
`Node::SelfCall`, which re-runs the current arm *without resolving the callee*. So redefining
a *called* global reaches a running loop (verified: it returns the new value), while
redefining the *looping function itself* does not — the loop keeps its old body until it
returns, and only a fresh call gets the new one. This is exactly Erlang's local-vs-remote rule
and it is correct by design, but `live-editing.md` claimed "the loop … picks up new code via
late binding", which is false in that case. Corrected there with both measurements, and it is
also *why* Stage 6 (an upgrade hook for long-lived processes) exists.

**Gates:** full suite green, `nest check` clean (one pre-existing advisory in a JIT torture
test), metamorphic differential fuzzer 420 checks / 0 divergences / 0 crashes.

## 2026-07-30 (cont.) — automatic macro binding hygiene (ADR-066 amendment, "Option A")

Made binding hygiene the **default**: a quasiquote template's own `let`/`letrec`/`fn` binders
(plain literal symbols) are alpha-renamed to fresh gensyms by the expander, so a macro's temp can
neither capture nor be captured by spliced caller code **without** `x#`/`(gensym)`. The
`(let (r ~a) (if r r ~b))` capture trap is now safe as written; `(let (r 99) (my-or false r))`
returns 99, not false. `#`/`gensym` still work (redundant). Anaphora opts OUT with `~'name`
(`aif`'s `it`). Retired the advisory capture lint (`types/check/hygiene.rs`) — it would only
false-positive now.

Why this is cheap where full Scheme hygiene (Option C, rejected on perf) is not: free-reference
hygiene (concern #1) was already automatic via the auto-qualifying resolver (ADR-065 §7), so the
*only* remaining capture vector is a template's own binders — a structural alpha-rename, no fat
`Value::Sym`, no cross-process cost. Mechanism (`eval/macros.rs`): `hygiene_rename` +
`hyg_walk`/`hyg_let`/`hyg_fn`, a **scope-aware** pre-pass in `expand_quasiquote` that renames only
the references a binder actually binds (so a same-named prelude reference is untouched — correct
even when a binder shadows a prelude name). A template that introduces a renamable binder takes
the runtime expand path (gated by `template_introduces_binder`), like `#`, so nested expansions of
one macro (`(m (m x))`) get distinct binders. GC-blocked like `resolve`. v1 renames only
plain-symbol `let`/`letrec`/`fn` binders; destructuring/`match*`/computed binders stay literal (a
sound under-approximation — never miscompiles a real macro). Migration was one macro (`defseq` →
`~'item`/`~'acc`, dropped `coll#`/`check-allow`). Tests: `tests/hygiene_test.blsp` (7 cases +
2 cross-process). Also cleaned three stale docs found en route: `deferred.md §6` (int/bool/string
literal precision shipped in B0, not deferred), the self-contradictory `of_value` comment
(`types/mod.rs`), and `namespaces.md` §2/§12 (privacy is enforced per ADR-146, not lint-only).

## 2026-07-30 (cont.) — exact rationals: a Brood-first prototype (`std/ratio.blsp`)

Ratios are on the freeze list as "refused, `1/2` token reserved" (ADR-169/170). Weighed the
two implementations: a pure-Brood record vs a kernel `Value::Ratio`. An ability/record gives you
arithmetic + display but **can't be a real number** — no `1/2` reader literal (a reader literal
must build a self-evaluating *kernel* value; Brood has no reader macros, ADR-150), not `=` to an
equal integer (structural map equality; there is no numeric-tower `=`), and `pr-str` prints the
underlying map rather than round-tripping. Those four properties (literal, `=`-with-ints, tower
ordering, round-trip) are exactly the kernel-only ones, and `Value::Decimal` (`0.5M`) is the
proof a numeric kernel type is a contained ~8-file change.

Per the dogfood-first rule (CLAUDE.md), shipped the **Brood prototype first** to settle the
reduce/gcd/sign/contagion design in the language before committing kernel surface: `std/ratio.blsp`
— `(rational n d)` builds a reduced, positive-denominator ratio record; `+`/`-`/`*`/`/` dispatch
through the `Num` multimethods (ADR-179, `[ratio ratio]` + `[ratio :int]`; `+`/`*` commutative
mirrors derived, `-`/`/` write `[:int ratio]` explicitly); `<`/`<=`/`sort` through `compare-to`
(`:antisymmetric`, cross-multiplied); `Display` prints `num/den`. All results renormalise, so two
reduced-equal ratios are structurally `=` and sort together; a ratio+float pair is a loud
`:no-method`. Embedded (opt-in, `system.rs`), `tests/ratio_test.blsp` (12 cases incl. cross-process
round-trip proving the record + method dispatch survive a `send`).

**Promotion criterion** (recorded so it isn't re-litigated): promote to a kernel `Value::Ratio`
(a near-clone of `Value::Decimal`) **iff** the prototype shows the kernel-only properties are
load-bearing in real use — the `1/2` literal, `=` with integers, and numeric-tower ordering/
contagion (incl. the ratio+decimal rule the decimal path leaves open). Until then the prototype is
the answer, and the freeze stays additive (ADR-169 reserved the token for exactly this).

## 2026-07-30 (cont.) — exact rationals promoted to a kernel type (ADR-196)

Promoted the ratio prototype to a kernel `Value::Ratio` (`num_rational::BigRational`), after it
proved the four kernel-only properties are load-bearing. `1/2` is now a reader literal, and **`/`
on integers is exact**: `(/ 1 2)` → `1/2`, `(/ 6 3)` → `2` (this overrides the freeze row
"`(/ 1 2)` is a float" — a discussed, deliberate relaxation; `->float`/`->decimal` are the escape
hatches). Full tower with int/decimal/float contagion (ratio+decimal is exact and lossless —
`(+ 1/2 0.5M)` → `1`); ratio arms in `value_cmp`/`equal`/`hash`; reader/printer/wire/message; both
GC collectors + region/checkpoint plumbing (mirrored on `Value::Decimal` at every site).
Normalize invariant: a denominator of 1 demotes to `Int`, so a ratio is never integer-valued
(`4/2` IS `2`).

**Perf note (the reason exact `/` is affordable):** `/` already returned int-or-float by
divisibility, and the JIT's unboxed loop already deopts on inexact division — so the hot path is
unchanged; only the inexact cold path now allocates a ratio instead of a float. The VM inline
`prim_apply` defers inexact `/` to `prim_div`; the JIT deopt lands there too.

Conversions: `numerator`/`denominator`/`->decimal` (kernel builtins), `->float`/`rational`/`ratio?`
(prelude). `number?` includes `:ratio`; the checker's `NUMBER` union too, so arithmetic/`<`/`sort`
type-clean. `sort`/`%sort-asc` widened to the whole numeric tower (fixed a latent decimal-sort
bug). The `std/ratio.blsp` prototype is deleted (superseded). Types/LSP/MCP all know `:ratio`.
Found + fixed a debug-only discriminant bound (`tag > 24` → `> 25`, dispatch.rs + jit/mod.rs) that
rejected the new tag. Tests: `tests/ratio_test.blsp` (14, incl. cross-process). En route fixed a
merge break: origin's receive-mark test called `receive_match` with the pre-`pin` arity.

## 2026-07-30 (cont.) — asking the build what it can do; a rounded `rect`; KI-21

Three gaps surfaced by writing a **downstream app** (`../waggle`, a browser for a
hypermedia protocol) rather than by working on the runtime — which is the point of
having one.

**KI-21 fixed** (`crates/nest/src/main.rs`). `nest run --for` and `--watch` wrap the
program in a generated `receive` whose source is a Rust string literal, and it still
carried a pre-ADR-150 pin: `([:down _ ~p reason] …)`. Since ADR-150 made `~`
quasiquote-only, **both flags failed on every file**, with a `match: \`~p\` is not a
pattern` error and no file/line. One character (`~p` → `^p`).

The lesson generalises past the typo: **Brood source generated from Rust string
literals is invisible to `nest check` and to the in-language suite.** Nothing could
have caught this except running the flag, and nothing did, because the wrapper is
only emitted when `--for`/`--watch` is set. Any such snippet needs an *execution*
test, not a reading — worth a grep for the others.

**`features` + `feature?` (ADR-197).** A builtin from an absent optional feature is
still *bound* and still raises when called, so `(bound? 'gui-open)` answers yes on a
runtime that cannot open a window. The app was reduced to calling it and matching on
the error's prose (`index-of … "gui backend"`) — which silently turns a graceful
terminal fallback into a crash the day someone rewords `NOT_COMPILED`. `(features)`
is one Rust builtin over `cfg!`; `feature?` is a prelude one-liner over it.

**`rect` takes a corner radius.** `frect` was rounded but GUI-only and `rect` was
square but universal, so rounded chrome needed *both* — an `frect` for the window plus
a `rect` inside it for the terminal, correct only because the two shared a colour.
`rect`'s optional 6th element rounds in the GUI and is ignored by the terminal, the
same asymmetry `cursor-zone` and `frect` already rely on. A zero radius emits the
5-element op unchanged, so every existing frame is byte-identical.

Verified: full in-language suite 3941 green, `display_test` + `introspection_test`
extended, `nest run --for 800ms` now exits 0 and prints `[stopped after 800ms]`.

## 2026-07-30 (cont.) — the std/ scale sweep starts: two quadratics in the framed reads

Picked up handoff thread 1 (the `std/` scale sweep, unstarted) on the reasoning that two of
the three quadratics fixed the previous session were in **Brood policy code, not the kernel**.
Both of today's are too, and both were in code whose own comments asserted it was linear.

**`tcp--read-until` was O(total²) in copy *and* scan.** It `bytes-concat`ed the whole
accumulator and rescanned it from offset 0 on **every** chunk — three lines under a comment
claiming both framed-read combinators concat "once, never per-chunk (no O(total²) rebuild)".
True of `tcp-read-n`, false of its sibling. And it has no size cap (the caller frames what it
reads), so a peer that drip-feeds and never sends the delimiter was a remotely-triggerable
CPU amplifier. Measured, 64-byte chunks: 250 → 9.3 ms, 1000 → 106 ms, 4000 → **1568 ms**.
16× the chunks cost **169×** the time.

**`http--read-until`'s ADR-142 fix was half a fix.** Threading a `from` offset made each byte
be *scanned* once but left `(bytes-concat acc d)` per chunk, which is the same O(head²) in
**memcpy** — the slow-loris amplifier had moved from scanning to copying, not gone away.
ADR-142 also claimed the chunk-list idiom made every `std/net` read path O(n) in copies;
neither `read-until` was. Corrected in the ADR.

**The fix, same shape in both.** Leave the reversed chunk list alone; carry the last
`(|sep|−1)` bytes forward as a **straddle probe** and scan only `[probe | chunk]` per chunk
(O(chunk)); concatenate **once**, on the delimiter, when the caller actually needs the bytes.
The cut offset comes from the running byte total plus the index into the probe, so it never
rescans. A match must include a byte of the new chunk (|probe| < |sep|), so nothing already
reported past can re-match. Result: **flat ~15 µs/chunk from 250 to 64 000 chunks** (4 MB
drip-fed in 64 000 chunks: 969 ms, where the old code would have needed hours).

**New harness `scripts/fuzz/stress/net_framed_scale.blsp`**, with the two controls the handoff
demands: `tcp-read-n` over the same drip (already O(total) — the reference for the accumulate
path) and a **floor** that receives the same messages and discards them (~1.0–1.7 µs/chunk,
pure mailbox cost). It needs no socket and no network: the combinators consume `[:tcp sock
data]` from the *mailbox*, and the clauses only pin `sock` by equality, so the drip is
fabricated with `send` to self — deterministic, no kernel read granularity or coalescing.

**`count` on a `bytes` value costs 514 ns; `byte-length` costs 36 ns.** `count` reaches bytes
at the bottom of a five-deep type-predicate `cond` in the prelude (`range?`/`seqview?`/
`string?`/`vector?`/`bytes?`). Switching the three per-chunk calls took the loop 22 → 15
µs/chunk. Reordering the `cond` is not the fix (whoever is last pays); the real answer is
type dispatch, which is thread 2 material. Worth knowing that `count` is not free.

**Refuted, before it became a theory:** that work inside a `receive` clause body runs on the
tree-walker. It doesn't — identical work in a clause body vs. called out to a plain function
measured 1424 vs 1466 ns. This looked live because a primitive timed at 375 ns standalone
appeared to cost 5.4 µs inside a receive loop; that gap was an artifact of the ad-hoc harness,
not of clause bodies. Re-measuring with every result consumed (in case dead pure calls were
being eliminated) reproduced the standalone numbers exactly, so that suspicion was unfounded
too. The residual ~15 µs/chunk is a **constant**, flat across a 256× range of backlog sizes,
so it is not the mailbox and not GC; it stays unexplained and is per-message-cost territory.

**Noise discipline:** the 16 000-chunk row has a ~15% spread over 5 runs (13.8–16.0 µs). The
`byte-length` win (22 → 15) clears that; a two-arg-vs-list `bytes-concat` tweak did not, so it
is kept for being simpler code and claimed as nothing.

**`make test` was broken at HEAD** — `mailbox.rs`'s capture-receive unit test still called
`receive_match` with 4 arguments after the receive-mark (ADR-195) added `pin`. Rust lib tests
therefore could not build, so last session's green run cannot have included them. Fixed.

**Contract pinned in tests:** `tcp-read-until`'s `rest` is only surplus from the chunk that
*completed* the delimiter — a later, still-queued chunk is not surplus. Three new cases cover
the straddle: a delimiter delivered one byte per chunk (every straddle slot in use), a
near-miss sharing the delimiter's prefix across a boundary, and a correct cut offset after 200
chunks. The first two initially failed on my own wrong expectations, not on the code.

**Still open in this thread:** `proc/agent update`, `buffer insert` and `buffer forward-line`
were swept clean in parallel with this work via `scale_sweep.blsp`; `proc/gen gen-call`'s ratio
is unstable there and needs three points plus medians before it means anything. `std/net/*`
beyond the framed reads, `editor/*` beyond buffer, and `std/tool/*` are still unswept.

**Gates:** `tcp`/`http`/`sse` suites green (79 tests), `nest check` clean apart from the one
pre-existing JIT-torture advisory. The rest of the suite is **red on merge, not on this work**:
`origin/main`'s in-flight exact-rationals commit (its own message says WIP) makes `/` return an
exact ratio where a float was expected — 7 nextest failures and 12 in-language ones, every one
of the shape `(/ 7 2)` → `7/2`, plus `%f64-sqrt: expected number, got ratio` (a real gap: the
float math builtins don't accept the new tag) and `types::tests::negation_and_difference`
asserting `number \ int = float ∪ decimal`, which the new `Tag` invalidates. This work's diff
is `std/net/*`, `tests/tcp_test.blsp`, docs, one harness and a 3-line comment — it cannot reach
`/`. Flagged for whoever finishes the rationals work; not fixed here.

## 2026-07-30 (cont.) — the rationals fallout: two real bugs, and the tower was never fully wired

Merging exact rationals (ADR-196) left the suite red — 7 nextest failures and 12 in-language
ones. All but two were stale expectations, and the two that weren't had been latent for much
longer than this session.

**Real bug 1: the float math family never accepted the numeric tower.** `expect_number`
coerces only `Int`/`Float`, and `floor`, `ceil`, `to-fixed`, `%f64-sqrt`, `atan2` and every
transcendental (`sin`/`cos`/`tan`/`atan`/`exp`/`asin`/`acos`/`ln`/`log2`/`log10`) used it — so
they rejected a **bignum and a decimal too**, and had done all along. Nothing hit it because
nothing routinely produced those in the middle of float math; making `/` exact did, and
`(sqrt (/ 200 3))` — an ordinary mean-then-sqrt, which is exactly how the telemetry summary
computes stddev — raised `%f64-sqrt: expected number, got ratio (200/3)`. They now go through
`num_to_f64`, the tower-aware coercion that already existed one screen away in the same file
for the arithmetic path. `floor` on a **ratio** is done *exactly* instead (`.floor()` on the
`BigRational`): since `/` is exact, `(floor (/ a b))` is an ordinary idiom, and via f64 it
would return the **wrong integer** past 2^53, not merely an imprecise one.

**Real bug 2: `round-to` returned a ratio for a float input.** It is
`(/ (round (* x scale)) scale)` — int over int, so now exact: `(round-to 3.14159 2)` came back
`157/50`. Rounding recovers no exactness a float never had, so a float `x` converts back. The
fix reproduces the pre-ADR-196 contract *exactly* rather than inventing a new one — that `/`
was float division, giving a float when it didn't divide evenly and an int when it did, which
is why `(round-to 2.5 0)` was `3`. Guarding on `(ratio? r)` keeps all four of those cases.

**A behaviour change kept rather than reverted: `pow` with a negative exponent.** An exact
base now gives an exact ratio (`(pow 2 -1)` → `1/2`), and it is *immune* to the underflow that
`pow--reciprocal`'s exponent-halving machinery exists to dodge — `(pow 2 -2000)` is exactly
`1/2^2000` where a float reciprocal is `0.0`. That machinery is now only needed for a float
base, and the test pins both halves including the `(pow 2.0 -1074)` → `5e-324` subnormal case.

**`=` is exactness-sensitive** — `(= 1/2 0.5)` and `(= 3 3.0)` are both false. Worth knowing
before reading any of these expectation changes: a function's return *exactness* is part of
its contract, so "it's still numerically 2.5" is not a defence. Where a test wanted the
inexact value it now says so with `->float`.

**Two things ADR-196 fixes for free**, both of which read as test failures first:
`(/ i64::MIN -1)` is the exact bignum 2^63 where the i64 fast path used to overflow into an
imprecise float, and an inexact division under the JIT's unboxed i64 worker deopts to a ratio
rather than a float. That second one was the only failure that could have been a *miscompile*,
so it was checked before being edited: JIT, `BROOD_NO_JIT=1` and the tree-walker all return
`212/3`, and `(/ 24 5)` is `24/5` on all three. The worker still bails out of its register —
just to a different heap value.

**ADR-196 had no section in `decisions.md`.** It went 195 → 197 while ten references pointed
at it, including a *superseding* note inside ADR-169 — so the reader ADR-169 sends looking for
"what replaced this" found nothing. Written up now from the shipped behaviour and the
reference docs, and marked as reconstructed: the author should correct the rationale.

**Also corrected: two stale claims about `sqrt`.** `docs/language.md` and `math_test`'s header
both said it is "computed in Brood (Newton's method), not a hardware sqrt" and is approximate.
The prelude's own comment says the opposite — it delegates to `%f64-sqrt` for IEEE
correctness, precisely *because* Newton's initial guess underflowed on subnormals.

**Gates:** `make test` **919 passed, 0 failed** (was 7 failed + a panicking in-language suite),
`nest check` clean but for the one pre-existing JIT-torture advisory, `cargo fmt --all` clean.

## 2026-07-30 (cont.) — LSP: type-directed record-field completion ships

The deferred ROADMAP item ("the hard part is knowing which record the value is — it
needs the checker's inferred type threaded into the completion path") is done, in
exactly the shape the deferral predicted: the context detection was easy CST work,
and the wiring became one new public checker entry point.

**`check::arg_ty_at(heap, forms, line, col, arg_index)`** — a position-keyed type
query. It arms a thread-local capture (`check/walk.rs`), runs the full `check_file`
analysis discarding the diagnostics, and returns the inferred `Ty` of item
`arg_index` of the call form recorded at `line:col`. Running the *real* walk is the
point: a same-file `defrecord`'s ctor `sig` (registered from the expanded
`%register-sig` forms), `let`-bound RHS types, sig-typed params, guard narrowings,
and Gap A global value types are all in force at the capture site, so `(let (p
(point 1 2)) (get p :…))` resolves `p` with zero new inference machinery. The query
is keyed by the **call form's** reader position, not the argument's own, because the
interesting argument is typically a bare symbol and the form-pos table is
pair-keyed — a symbol carries no position. A macro-duplicated position keeps the
capture open until something yields a type (`rebuild_list` copies positions).

**The completion side** (`crates/lsp/src/completion.rs`): `record_key_context`
classifies the cursor's argument slot off the CST — key positions of
`get`/`update`/`contains?` (slot 2), `assoc` (even slots ≥ 2), `dissoc` (≥ 2), and
the keyword-accessor head `(:… m)` — and only while the slot is still
keyword-shaped, so a computed key `(get p k)` stays with the generic candidates.
Because completion happens mid-edit, the buffer is repaired before the strict read:
the partial key token is blanked **in place** (byte-for-byte spaces, so every
offset survives; a lone `:` classifies as a *symbol* and wouldn't read), and
`close_open_delimiters` appends the missing closers, string- and comment-aware.
Fields arrive as `:keyword` items (kind FIELD) carrying the declared field type as
detail; `__id__` is skipped; every miss — unparseable buffer, unknown type, not a
record — degrades to no extra candidates, never a wrong list. `:` joined the
trigger characters so the popup fires the moment a key is started.

Deliberately not offered: fields inside a bare **map literal** (the literal under
construction has no identity to infer from; typing it from an *expected* parameter
type needs bidirectional checking) — and the cheap "offer every defrecord's fields"
heuristic stays rejected as noise. Tests: 4 checker-side (`arg_ty_at` on ctor
args / let-bound / Gap A globals / miss-degradation), 6 LSP-side (mid-edit lone
`:`, let-bound, typed detail, keyword-accessor head, wrong-slot silences, the
delimiter closer). Also fixed while there: `docs/lsp.md` still described KI-16
(fixed 2026-07-27) as open.

## 2026-07-31 — the overnight soak, and why nine hours does not fit

Set up the unattended endurance run the handoff had been asking for (thread 5). The first
thing it produced was a reason it could not be run the way it was specified.

**RSS under sustained churn grows ~1 KB/iteration and does not plateau — with OR without
`MIMALLOC_PURGE_DELAY=0`.** Measured with purge delay 0: 24 → 270 → 474 MB at 0 / 200k /
400k iterations. On the default allocator, ~34 MB/min armed and ~73 MB/min control,
near-linear in time. At ~670 it/s a 9-hour run of `soak_selfcheck.blsp` needs roughly
**21 GB**, so every configuration would have been OOM-killed partway and left a truncated
log — the weakest possible evidence for the one question the run exists to answer.

This is not a new leak: A8 already established that RSS is allocator retention against a
live set of tens of KB, and that finding still holds. What is new is the **rate**, and that
the documented mitigation only slows it. **It turns thread 3 (allocator policy) from a
preference into a constraint:** as things stand, no available allocator configuration lets
this workload run unbounded overnight. A long-lived server doing this much churn per second
needs an answer, and "spend memory for speed" is not one at 1 KB/iteration.

**So the night runs as a sequence, not a marathon:** repeated 30-minute soaks (the duration
already validated at ~1.5 M iterations), alternating armed and control, each reaching a
definite `OK soak complete` with memory released between runs, halting the whole sequence on
any `ERROR`. Plus one default-config run left going deliberately to measure the growth curve
to its 6 GB cap, since where it dies is worth knowing precisely. Every run sits in its own
`systemd-run --scope -p MemoryMax=6G`, so a runaway is killed inside its own cgroup instead
of destabilising a 30 GB desktop. Results, and how to read them:
`~/brood-soak-2026-07-30/README.md` → `sequence.log`.

**The detector was verified before being trusted**, per the standing rule: draining one
message fewer than queued gave `ERROR at iteration 0: backlog lost: saw 63 of 64` and exit 1
on this exact binary.

**Two process notes.** `SOAK_REPORT` is in *iterations*, so heartbeats land at equal
iteration counts and `rss_kb` is comparable across runs at equal `iter=` — the property that
sidesteps the time-boxed-differencing trap; compare at equal `iter=`, never equal `t=`. And
the `pkill -f` trap in CLAUDE.md caught me exactly as documented: `pkill -f "mirror[.]sh"`
killed the shell that was writing `mirror.sh`, because the bracket trick protects the
*pattern*, not the rest of the command line. Kill by PID.

**First sequence run: `OK soak complete: 813534 iterations, rss_kb=869468 rt-closures=72`** —
zero invariant violations, and run 2 (control) auto-started, so the harness works end to end.
It also showed a **second** effect: throughput decayed *within* the run, 2041 → 633 → 362 →
259 it/s across successive 200k-iteration segments while RSS went 28 → 923 MB — ~8× in 29
minutes. That is confounded (the growth-curve run was competing for CPU and memory bandwidth
the whole time, and was itself growing; a solo 25 s run did 3537 it/s), so it is recorded as
an observation, not a result. It disentangles for free later in the night: once the
growth-curve run hits its 6 GB cap the remaining sequence runs are effectively solo, so a
late armed run's opening rate against run 1's 2041 it/s separates contention from intrinsic
decay with no new experiment. If it is intrinsic it matters more than the RSS growth does —
a server that slows 8× in half an hour of churn is a worse problem than one that holds a
gigabyte.

## 2026-07-31 — soak result: 12.7M iterations clean, and an 8× throughput decay a restart cures

The overnight sequence finished: **16/16 runs `OK soak complete`, 12,671,363 self-checking
iterations, zero failures** (8 armed ~819k each, 8 control ~765k each, over 8 hours).
Handoff thread 5 is closed — the runtime stays *correct* under sustained load overnight.

**First, a correction to last night's entry.** I claimed a single 9-hour run "needs ~21 GB"
and would be OOM-killed, and restructured the night around that. The projection was wrong: it
extrapolated linearly *in time* from the first 400k iterations (~670 it/s), but throughput
decays ~8× within a run, so a long process does far fewer iterations than a constant-rate
projection assumes. The long run left going all night reached **3M iterations and 3.06 GB in
7 hours** — well under its 6 GB cap. **A single 9-hour run would have fit.** This is A8's own
trap in fresh clothes: I reasoned in MB/*minute* when the driver is MB/*iteration*. The 21 GB
figure should not be quoted; the sequence was still the better experiment, but not for the
reason I gave.

**RSS is ~1.0 KB per iteration and deterministic in iteration count, not in time.** Run 1
(00:07) and run 15 (07:07) hit 354/382, 526/527, 728/690, 923/921 MB at the same four
iteration marks, seven hours and ~11 M intervening iterations apart.

**The main finding: a process degrades ~8.3× as it churns, and a fresh process is fully
restored.** Run 1 and run 15 both went 2041/2174 → 633/673 → 362/352 → 259/263 it/s across
successive 200k segments — the same curve, so last night's "is this just sibling contention?"
caveat is answered: it is **intrinsic**, driven by the process's own accumulated RSS. The
long-lived run shows it compounding — its three successive millions took 2988 s, 8453 s,
13958 s (335 → 118 → 72 it/s). But run 15 ≡ run 1 means there is **no cross-run degradation
at all**: nothing accumulates outside the process, and a restart recovers everything.

Per-process and reversible is consistent with allocator/page-locality decay against a live
set of tens of KB — not a leak, and not runtime-region growth (`rt-closures` stays ~70 on
every armed run all night). `MIMALLOC_PURGE_DELAY=0` does **not** fix it (its probe decayed
1493 → 430 it/s over 400k). Root cause is not established and is not worth guessing at; this
is the strongest lead the soak produced. **It reframes thread 3**: the allocator question is
no longer "how much memory do we spend for speed" but "why are we *losing* 8× of speed to
retention, and why does only a restart give it back". For a long-lived server that is a
bigger problem than the footprint.

**ADR-194 attribution, consistent across all 8 pairs:** armed ~819k iterations / ~915 MB /
`rt-closures` **~70**; control ~765k / ~2.05 GB / `rt-closures` **~760,000**. The control
copies every closure across a send, so the append-only RUNTIME region grows by ~760k closures
per run for 7% fewer iterations. Two control runs also showed `rt-threshold` off its 4096
default (1455526, 562170) — ADR-091 shared-region reclamation engaging, visible only there.

Full write-up, logs and the per-minute `rss.csv`: `~/brood-soak-2026-07-30/README.md`.

**Loose end from the same run: `brood::suite` came back `FLAKY 2/2`** — the in-language suite
failed once and passed on nextest's retry, so `make test` still exited 0 and reported "929
passed". Three subsequent full `nest test` runs were clean (3978/3978 each), so it did not
reproduce, and the failing assertion was **lost because the run was piped through `tail -12`**
— nextest prints the first attempt's detail above the summary. Not chased further and not
assumed benign. The likely shape is one of the timing-sensitive cases (the `tcp` idle-timeout
family sleeps 500–2500 ms against `after` windows, and the cargo suite runs under far heavier
parallel load than `nest test` does), but that is a guess, not a finding. Next time the suite
is run, capture the whole log — a green "N passed" line can be hiding a retried failure.

## 2026-07-31 — the CI type-checker gate reaches zero: 60 warnings fixed or justified

The one red step in CI was the deliberate hard gate — `nest check std/**/*.blsp
tests/**/*.blsp` at zero warnings ("a soft gate that is permanently red is noise, and
the point is to drive the count to zero"). It reported 60: 58 `recursive call in
non-tail position`, 2 `gui-open` argument types. All are now fixed or carry a
justified opt-out, and the gate is green.

**Two real fixes.** `gui-open`'s Sig declared `(string int int map)` params, but the
primitive deliberately accepts `nil` in every optional slot ("use the default") — the
callers were right and the signature was narrow, so it now says
`string|nil int|nil int|nil map|nil`. And the bootstrap `append--2` (the quasiquote
builder seed that predates `defmacro` itself, so no `check-allow` can reach it)
became genuinely tail-recursive via a double reverse-onto — the same shape as the
full `append` that later rebinds it, so a deep append during bootstrap now costs
O(1) stack instead of a frame per element.

**56 justified opt-outs.** Every remaining site is deliberate bounded recursion, now
wrapped in `(check-allow :non-tail-recursion …)` with a one-line justification of
the actual bound — the first uses of the opt-out in `std/` (it had only appeared in
tests). Three families cover most of them: the `match-*` pattern compiler (13) and
`receive--*`/`binding--*`/threading/`for--fold` builders (11) recurse over **macro
syntax at expansion time**, so depth is source nesting, never runtime data; the
editor's `pane--*`/`keymap-bind`/`face--resolve`/`lineedit--*`/`ts--chain-rest`
walk trees whose depth is screen splits / key-sequence / inheritance-chain length;
and the genuinely-data-shaped ones state their real bound (`merge-sort--n` is
log₂ n, `assoc-in`/`dissoc-in` are path length, `flatten--acc` is nesting depth,
`regex--*` is pattern nesting — never subject length). The JIT effect-once torture
fn keeps its non-tail shape on purpose and says so.

Also swept while red: a `clippy::nonminimal_bool` in `hyg_renamable`
(`eval/macros.rs`) that had newly broken the clippy step. The CI workflow's stale
"currently 81 warnings, this step FAILS" note now records the zero and the rule
that keeps it there: a new warning is either a real finding or a missing justified
opt-out.

## 2026-07-31 (cont.) — thread 6 diagnosed: the sink is the append-only RUNTIME region

Took the throughput decay. It is not the allocator, and it is not diffuse: it is one code
path, with one sink, and the reclamation policy for that sink costs **45% of throughput** on
the workload that feeds it. New harness `scripts/fuzz/stress/decay_isolate.blsp`.

**Step 1 — isolate the operation.** The soak does five things per iteration, so the first
question is *what* decays, not why. Each mode runs one operation in a tight loop and reports
throughput per fixed-size window. Flat, over millions of ops each: `alloc` (52 M CHAMP
assoc/count, RSS plateaus at ~70 MB and ends *lower* than it started), `cons` (74 M),
`spawn` (55 M), `sendrecv` (50 M), `roundtrip` (11 M), `backlog` (1.4 M). So the allocator,
the GC, plain spawning, message send/receive and the selective-receive scan are all
**exonerated** — a useful result on its own.

The one that decays is the **supervisor child cycle**: 740 k ops took RSS from 91 MB to
1.31 GB (~1.7 KB/op) and throughput from 19 607 to 9 900 ops/s. ~1.7 KB/op matches the
soak's ~1.0 KB/iteration, and the soak does one supervisor cycle per iteration.

**Step 2 — four refuted hypotheses, each cheap.** The restart-intensity window: `:max-seconds
1` is indistinguishable from `60` (1.37 GB vs 1.31 GB, same curve) — not it. The crash: three
supervisor-free modes (`spawn-link` + normal exit, + `error`, + `throw`, 3–5 M ops each) are
all flat, so `spawn-link`, exit signals and error values are clean. Closure nesting in a
message: sending a bare fn, `{:start fn}`, `[fn]` and a fresh anonymous fn 100 k times each
grows `:runtime-closures` by **1–2 total** — ADR-194's share path is fine, including nested.
Who spawns: root-spawns-200k vs spawned-worker-spawns-200k both flat.

**Step 3 — the sink.** Reporting `:runtime-closures` alongside RSS pinned it in one run.
Over 390 k supervisor cycles it climbed **2 583 → 341 592**, monotonically, ~1 per cycle,
while the supervisor's own heap oscillated (0.16–2.5 MB) and the caller's live bytes did too
— both LOCAL heaps collecting normally. Per-call granularity: `start-child` is **+1**,
`terminate-child` is **+0**. So each supervised child start appends a closure to the
**append-only shared RUNTIME code region**, and terminating the child reclaims none of it.
`supcall` (a supervisor round-trip that starts nothing) is flat at 64 over 2.25 M ops, so it
is the child start, not the call.

**Step 4 — the reclamation policy is the expensive half.** `BROOD_RT_GC_FLOOR=100000000`
(never compact the region) does **45% more work**: 610/610/610 k ops per 30 s against
420/420/430 k on the default floor of 4096 — three runs each, ~2% spread within a condition,
so far outside noise. Compacting *more* often (`=256`) does not help either. The region grows
regardless; what the default policy buys is repeated attempts to reclaim something it cannot
reclaim, and that is where the throughput goes. This is the **KI-14 class resurfacing** — that
was "the RUNTIME collector re-walked a deep process's whole root stack at every safepoint, so
cost scaled with loaded code, not test size".

**Why the soak's fresh processes looked fine:** the region is per-runtime, so restarting the
OS process resets it. That is exactly why run 15 matched run 1 while a single long-lived run
decayed — the "restart cures it" observation was this all along.

**Not fixed, deliberately.** Two candidate fixes and both are ADR-091 design decisions, not
mechanical: (a) stop promoting per `start-child` — if a supervised child's start thunk is
already shared, the spawn should not append a fresh entry; (b) make the reclamation threshold
adaptive — back off when a compaction reclaims little, instead of retrying at a fixed floor
and thrashing. (b) is worth 45% on this workload by itself. Both touch the GC, which is where
this repo is most careful, so they want a deliberate decision rather than a drive-by patch.

## 2026-07-31 (cont.) — the formatter mangled every ratio and decimal literal

Told to run `nest format` tree-wide and push it. The diff was wrong in a way that is not a
style verdict, so it did not get pushed — the formatter had a real bug, and `nest format
--check`'s red was partly this rather than the documented hoisting disagreement.

**Every form containing a ratio or decimal literal was force-broken.**
`(assert= (sqrt 9/4) 1.5)` came out as four lines; `(f 9/4)` became two. `(f 1.5)` was fine,
so it was not width and not hoisting — the tell was that every mangled line held a `1/2`,
`9/4`, `7/2`, `1.5M` or `4.0M`, while `(floor (pow 10 30))` beside it was untouched.

**Cause: two hand-maintained copies of the leaf-kind list, and only one learned ADR-196.**
`single-line` enumerated the CST leaf kinds and fell through to `else nil`, which means
"cannot appear on one line" and forces the enclosing form to break. `render` has the *same*
enumeration but falls through to `(node-text n)`. So when ratios and decimals were added to
the CST, `render` kept emitting them correctly and `single-line` silently started reporting
every form containing one as un-inlineable. Correct output, ruined layout — which is why it
survived to a tree-wide run.

**Fix:** one definition, `*verbatim-kinds*` (a set) + `verbatim-kind?`, used by both. Written
as a set rather than the `or` chain because the `or` chain is what drifted, and because the
formatter reflows a 9-clause `or` into something worse than the set literal.

**Worth 276 lines of the tree-wide diff.** Before: 42 files, +823/−547 (net **+276** lines of
explosion). After: 43 files, +795/−799 (net **−4**). So a meaningful part of the "26 files
red" the roadmap treats as a style verdict was this bug. The rest genuinely is the hoisting
question, and that still needs a human call.

**Tests: four cases, verified to fail first.** One per leaf kind the reader can produce, plus
the contract that actually protects the tree — formatting must not *grow* an already-canonical
file. Confirmed all four fail on the unfixed formatter (7 failed assertions) and pass with it;
a test never seen to fail is not a test.

**Gates:** `nest test` 4077 passed, `nest check` clean.

**A second fix attempted and reverted, with the measurement.** The rest of the tree-wide diff
is the formatter breaking a generic call after exactly one argument and then filling from a
fresh line, so `(error "a long string " x " more " y)` puts the string alone on the head line
and packs the remainder beneath it — a break at an arbitrary point, which is most of why the
formatter disagrees with hand-written multi-arg `error`/`str`/`println`. Seeding the fill
column from the head line's end fixed those two cases exactly (they came out byte-identical to
the hand-written source), and then made everything worse: **199 files / +2085 −3222 (net −1137
lines)** against the 42-file baseline, because filling from the head line also collapses lines
the author deliberately broke — it fights the `had-author-newlines?` rule that is the whole
reason the formatter is idempotent and respects line structure. The one-arg-then-fresh-line
shape is deliberate. Reverted; the tree-wide diff is back to 42 files / +779 −796.

So the residue really is the style verdict `roadmap-for-v1.md` describes, now smaller and
better understood: **98 lines of it are comment hoisting** (a 1:1 conversion of aligned
trailing comments to own-line comments — exactly 98 removed lines carried a trailing comment),
and the remainder is call-breaking that reads worse than the source but cannot be improved by
filling without losing author line structure. That still needs a human decision, and the
formatter is not going to settle it.

## 2026-07-31 (cont.) — comment hoisting dropped: the formatter leaves a trailing comment alone

Settled the `nest format --check` style question in the direction the evidence pointed: the
formatter changes, not the tree. A same-line trailing comment now stays **where the author put
it** instead of being hoisted onto its own line above the form. `roadmap-for-v1.md` had already
identified hoisting as "the entire disagreement" — the tree's authors do not write that way —
and it destroyed the alignment that makes a column of trailing comments readable.

**Four emit sites, not one**, which is why the roadmap expected this to simplify things:
`render--body--at` (body forms), `render--pair-walk` (`let`-family bindings),
`render--body-pairs--at` (`cond`/`case`/`assoc` clauses — which had *no* same-line handling at
all, so those comments always went to their own line), and `format-cst-root--walk` (top level).

**Two consequences worth naming.** `last-nonws-comment?` got simpler exactly as predicted: a
last comment now forces the close delimiter to its own line unconditionally, because leaving
comments in place means a comment that is last in the source is last in the output — the
`comment-on-own-line?` distinction it needed is retired. And the same rule became **load-bearing
in a second place**: `render--pair-break` appended the bindings list's `)` with no such check,
which was safe only *because* hoisting guaranteed a comment could never be last. Without it,
`(let (a 1 ; the a` … `b 2 ; the b)` put the `)` inside the comment — an unparseable file. Found
by the idempotency probe, fixed, and pinned by a test.

**Result: hoisting is gone from the tree-wide diff — comment-only added lines 98 → 15**, and the
sweep now *removes* 96 net lines instead of adding 17. The file count is unchanged at 42,
because the residue is the call-breaking shape, which the earlier reverted experiment showed
cannot be improved by filling without discarding author line structure.

**Verified against the whole tree, not just unit tests.** Ran the sweep, then built and ran the
suite on the formatted result: 43 files rewritten under the new rules — `std/prelude.blsp`
included — **4079 tests passed**. Then reverted the sweep; this commit is the formatter change
only. Six hand cases (body, last-form, bindings, bindings-ending-in-comment, cond pair,
top level, own-line) all check out and all are idempotent.

**Gates:** `nest test` 4079 passed, `nest check` clean, formatter suite 72 passed.

## 2026-07-31 (cont.) — thread 6 is the JIT re-promoting a capture-free closure per activation

Kept going on the decay and found the mechanism. It is not the allocator, not the supervisor,
and not the reclamation threshold — those were symptoms. **The JIT (and the tree-walker) rebuild
and re-`promote` a capture-free closure on every evaluation, appending to the append-only
RUNTIME region; the bytecode VM does not.** Minimal repro: 20 000 `start-child`/`terminate-child`
cycles.

| engine | `:runtime-closures` after 20 k cycles |
|---|---|
| default (VM + JIT) | 11 969 |
| `BROOD_NO_JIT=1` (plain VM) | **58** |
| `BROOD_VM=0` (tree-walker) | 15 989 |

**On the real harness the difference is enormous.** `supnocrash`, 20 s, same binary:
default does **300 k ops / 639 MB RSS** with `rt_closures` 2 191 → 10 273 and throughput decaying
19 083 → 16 611 ops/s; `BROOD_NO_JIT=1` does **590 k ops / 318 MB RSS** with `rt_closures`
**73 → 75** and throughput *flat* (27 777 → 28 901). So **1.97× the work, half the memory, no
region growth, and the decay itself disappears.** That is the whole of thread 6, and it also
explains why a restart cured it (the region is per-runtime) and why every non-supervisor mode
measured flat (they don't create a closure per op).

**Why:** `make_closure_cached` exists precisely for this — its own comment says a capture-free
closure "is a *constant*… Re-building and re-`promote`ing an identical one every evaluation — a
`spawn` thunk in a fan-out — piles up RUNTIME garbage". But it caches **only when `fn_rest` is a
RUNTIME pair**; anything else falls through to `parse_closure_template` + `build_closure`, which
promotes. Both VM engines reach it with a RUNTIME `fn_rest` (`exec_value.rs:456`,
`exec_chunk.rs:846`) and get the fast path. The other two engines don't, and the bail is silent.

**Correcting two earlier claims of mine.** (1) The 45% attributed to `BROOD_RT_GC_FLOOR` was
real but it was treating the symptom — stop collecting a region that shouldn't be growing.
(2) I reported "the root's `rt_threshold` pinned at 4096 proves thrashing"; the root never
promotes, so that field is merely stale. Neither reading was right about the cause.

**Also tried and reverted:** routing the tree-walker's `fn` through `make_closure_cached`
(`eval/mod.rs:590`, the one call site using the raw builder). A **no-op** — still 15 989 —
because the tree-walker's `fn_rest` is a LOCAL pair, so the cache bails exactly as above.
Reverted rather than left in with a confident comment.

**Not fixed, and this one genuinely needs a design decision.** The fix is to make the cache
reachable when `fn_rest` is not RUNTIME, and `make_closure_cached`'s own comment explains why
that is not a one-liner: a LOCAL handle's slot can be reused by a minor GC *without* bumping
`gen_version`, so a LOCAL key is unsound to cache on. Needs a sound key (or a different place to
memoise the constant) — ADR territory, in the collector's blast radius. Worth doing: it is
~2× throughput and ~2× RSS on any workload that spawns per request, which is every server.

## 2026-07-31 (cont.) — the suite flake, finally caught: two of them, one fixed

The `brood::suite` `FLAKY` marker from yesterday recurred, and this time the full log was kept
(the earlier run was piped through `tail`, which is what lost it — nextest prints the failing
attempt *above* the summary). Two distinct flaky tests, not one.

**Fixed: `tests/private_test.blsp` had no `:serial`.** Every test in its one `describe` block
`%load-string`s whole **modules** into the shared module registry and global table — 16 of them
— and the alias test reads `priv-vault`, which a *different* test registers. Run in parallel
that is a straightforward race, and it surfaced as `require: cannot find module 'priv-vault2'`
roughly one suite run in five. Marked `:serial`, the same reason `suite_test.blsp`'s macro and
loop-macro blocks are, one level up: module registration, not just a `defn`. 13/13 standalone
and clean across three full suite runs after.

**Identified, NOT fixed: `tests/ability_test.blsp:158`** ("a reload is picked up even after the
cache is warm"), which failed once in those three runs. That block is **already `:serial`**, so
serialising within a `describe` is not enough — the test mutates the global ability registry and
the `%dispatch` cache epoch, and sibling test *files* run concurrently with it. It needs
isolation at a level `:serial` doesn't give. Deliberately left alone: validating a 1-in-3 flake
takes ~10 suite runs, and a guessed fix here would mask it rather than remove it.

**The process lesson, which cost two sessions:** a green `N passed` line can hide a retried
failure, and `make test | tail` throws away the only evidence. Keep the whole log.

## 2026-08-01 — std/ scale sweep: `write-lines` was O(total²), and so was `nest doc`

Resumed the sweep (handoff thread 1) with the shape the net findings taught: **grep for a
`str`/`append`/`concat` whose argument is the accumulator.** That one grep over `std/` found
both of today's.

**`file/write-lines` — a public std API, quadratic.** It built its output with
`(fold (fn (acc l) (str acc l "\n")) "" lines)`, recopying the whole accumulator per line:

| lines | before | after |
|---|---|---|
| 2 000 | 37 ms | 0 ms |
| 4 000 | 144 ms | 0 ms |
| 8 000 | 361 ms | 0 ms |
| 16 000 | **1888 ms** | **1 ms** |
| 64 000 | — | 5 ms |

One `join` (native `%string-join`, one pass into one buffer) plus the trailing newline. The
empty case needs a guard — `(join "\n" ())` is `""`, and an unguarded trailing newline would
write a one-byte file where the fold wrote an empty one. Four tests pin the edge cases (single
line, empty, vector-vs-list seqable) plus a linearity tripwire.

**`nest doc` had the same shape three times over, nested.** `docs--section-defs` folded
`(str acc (docs--entry …))` per definition, `docs--section-vars` per variable, and
`document-project` folded `(str acc (document-file f))` per *file* — the outermost one, so
generating a project's docs was O(total-markdown²). All three are now `(apply str (map …))`,
joining once. `nest doc` verified end-to-end on this repo.

**Checked and deliberately left:** `template--loop` (`(str acc before val)` per `{{key}}` — same
shape, but a template is small and the placeholder count doesn't scale with anything) and
`project.blsp:1683`'s `(append a (list …))` per source file (O(files²) but **spine only**, and
files are in the hundreds: ~90 k cons copies at N=300). Noted rather than churned.

**Gates:** `nest test` 4083 passed, `nest check` clean.

**Second shape swept, and it came back clean.** After the accumulator grep, I swept the other
classic quadratic: an **O(i) accessor inside a loop** — `(nth coll i)` where `coll` is a list or
a string, since string indexing here walks to the char boundary (the same reason `format.blsp`
warns that a per-char `substring` loop is O(n²)). Every hot candidate is already handled:
`encoding`'s hex and base64 decode loops convert with `string->codepoints` first ("codepoint
vector: O(1) nth" in their own comment), `regex`'s per-character matcher indexes a codepoint
vector and its NFA `:states` is a vec, and `telemetry`'s bucket bounds are small and fixed.
Recording the negative so the next sweep doesn't re-walk it: **the shape to keep hunting in
`std/` is the accumulator one, not the accessor one.**

## 2026-08-01 (cont.) — the suite flake is a real dispatch race (KI-22), not test hygiene

Chased the remaining `brood::suite` flake to a definite answer, and it is a runtime bug.

`tests/ability_test.blsp`'s "open extension" registers an impl and calls the op on the next
line; `(esize [1 2 3])` intermittently returns `-1`, the `:default` impl. Roughly one suite run
in five to eight.

**The obvious theory was wrong, and disproving it is the result.** The test extended the shared
`Size` ability, which several NON-serial blocks in the same file dispatch on — and `:serial`
only orders tests *within* a block, so a cross-block collision looked certain. I moved the test
onto a private `Extend` ability that nothing else in the tree touches. **It still failed, on run
8 of 10.** So this is not test hygiene: an `impl` registration is not reliably visible to a
dispatch on the following line while other processes register unrelated impls.

That points at the shared `%dispatch` inline cache / registry (ADR-172 §7) — the same invariant
the `dispatch cache is transparent` block asserts and passes deterministically. Filed **KI-22**,
open. It matters past the test: `impl` is hot-reloadable by design, so a live reload that can be
missed by the next call is a *wrong answer*, not a crash.

The private-ability change is kept, with its comment corrected to say it did **not** fix the
flake and what it rules out — an isolated repro is worth more than the original tangled one.
Left the test un-serialised on purpose; re-serialising would hide the bug.

**Also worth recording, because it cost three sessions:** nextest retries, so this printed
`929 passed` with only a `FLAKY` marker, and the failing attempt is logged *above* the summary —
`make test | tail` threw away the one thing needed. Keep the whole log.

## 2026-08-01 (cont.) — KI-22's root cause found; both fixes reverted

Ran the flake to ground. It is **a lost update, not a dispatch-cache bug**, and it is much
bigger than the flake suggested — but I reverted both attempted fixes, so the bug is still open.

**Root cause.** Every load-time registry is one global holding a whole map, written as
`(def *X* (assoc *X* …))` — a read-modify-write. Two processes registering at once each read the
old value and each write their own successor; the later write drops the earlier one. A direct
probe (N processes, one *private* ability each, so no legitimate precedence contest) measures
**24/50, 88/200, 218/500 lost — about 40%.** Preserved as
`scripts/fuzz/stress/registry_race.blsp`: fast and deterministic, unlike the 1-in-5 suite flake.
**Fifteen registries share the shape**, so multimethod registration has the identical bug to
ability registration and nobody had hit it yet.

**Why both fixes came out.** Optimistic retry (write, re-read, retry) cut the loss 44% → 20% but
cannot close the read-write window; a partial fix for a wrong-answer bug only makes it rarer and
harder to find. A `table-incr` ticket lock did reach `LOST=0` at every size and took the suite
from failing on run 8 of 10 to 10/10 clean under `nest test` — and then `make test` came back
**worse than the bug**: the regression test burnt **157 s** of CPU and *still* lost one, because
a bounded busy-spin is blown through under load and the waiter then proceeds unsynchronised.
Sleeping between checks fixed the CPU burn and exposed the next flaw — a timed-out waiter never
bumps `:served`, desynchronising the ticket sequence permanently, so every later registration
pays the full timeout (a constant 20 s regardless of N).

Two lessons, both mine to keep: **`nest test` passing ten times proved nothing** — `make test`
runs the in-language suite against 928 parallel Rust tests and is the environment this class
shows up in; and a lock whose failure mode is "proceed unsynchronised" is not a lock, it is a
probability adjustment.

**Left open deliberately** rather than shipping a third half-validated concurrency fix into the
most contention-sensitive part of the system at the end of a long session. KI-22 now carries the
root cause, the reproducer, both dead ends, and the shape a real fix probably takes: a
**registrar process** (a blocking call to one single-threaded writer — no spin, no timeout, no
desync, zero lost updates by construction), whose one open question is bootstrap, since
registration happens during prelude load.

## 2026-08-01 (cont.) — KI-22 fixed properly: one kernel primitive, atomic by construction

Third attempt, and this one is right. `%registry-update!` performs the whole read-modify-write
of a registry global **inside a single kernel call**, under a per-runtime `registry_lock`.
Atomic by construction: no CAS (so no ABA question), no retry loop, no spinning, no callback
into Brood under a lock — every failure mode of the two reverted attempts is structurally
absent rather than mitigated.

Four ops cover all fifteen registries: `:assoc`, `:assoc-new` (the presence test is *inside*
the lock, so a derived method mirror cannot clobber an authored impl registered in between),
`:dissoc`, and `:cons-new` (the `member?` test inside the lock, for `provide`). Policy stays in
Brood — the call sites still say what they mean — and only the atomicity is kernel, which is
exactly the mechanism/policy split the repo asks for. **Reads are untouched**, which is the
constraint that ruled out `Table`s: dispatch reads `*impls*` on every call and a table
deep-clones values in and out, which would put a closure copy on the hot path.

**Results:** `registry_race.blsp` goes from 218/500 lost to **0 lost at 50/200/500/1000, in
0.1 s**. Suite green at 4097. Prelude registry counts back to their pre-change values.

**The trap that cost a cycle, worth knowing for any future kernel primitive standing in for a
special form:** `def` binds at `env_root(env)`, **not** unconditionally `EnvId::GLOBAL`. During
prelude load the root is a bootstrap env whose bindings later *seed* the shared runtime, so my
first version — writing straight to the globals table — had every prelude-time write silently
discarded at seed time. The prelude lost its own `Display`/`Inspect` impls and `to-str` failed
with "no impl for :string". Caught because `url_test` went red; diagnosed by building the
pre-change binary and comparing registry counts at startup (4 vs 0) rather than reasoning.

**Also fixed: the reproducer was lying about timing.** `(spawn (worker i (self)))` evaluates
`(self)` in the CHILD, so every worker messaged itself and the collector just timed out — the
"20 s" in the earlier measurements was the timeout, not the work. The loss numbers were still
sound (20 s was ample for the workers to finish), but the harness now passes the parent pid in
and runs in 0.1 s.

## 2026-08-01 (cont.) — `concurrently`: make a race a one-line test instead of a flake

Added `concurrently` to `std/tool/test.blsp`: `(concurrently n f)` runs `(f i)` for `i` in
0..n-1, each in its own process, blocks until all finish, and returns their results. The three
concurrency tests written today were each eight lines of hand-rolled spawn/join/count
scaffolding; they are now three lines apiece.

**It exists to make one specific mistake impossible.** `(spawn (worker (self)))` evaluates
`(self)` in the **child**, so every worker messages itself and the collector waits for messages
that never arrive. That bug was written twice in this repo's own stress harnesses today —
once in `registry_race.blsp` (where it masqueraded as a 20-second runtime cost that was really
just the collect timeout). `concurrently` captures the parent pid in the parent, once.

Results are collected on a fresh `ref` minted per call, so a stray message can never be
mistaken for a worker's result — and pinning that ref lets the receive-mark (ADR-195) skip an
unrelated backlog rather than walk it per result. A worker that wedges fails the test at
`*test-timeout-ms*` instead of hanging the run.

**The point is deterministic concurrency tests.** KI-22 cost three sessions as a ~1-in-5 flake
that only appeared under machine load; the test that finally cornered it catches the same bug
on the *first* run, at any load, because it creates the contention itself. That is the general
lesson: don't hope the suite's own parallelism interleaves the right way.

**A correction to something I said earlier in this log.** I claimed `make test` was "the
reproduction environment for this class" and implied `nest test` was inadequate. Both wrong.
`nest test` is the runner for Brood projects, and it already ships `--repeat-until-failure <N>`
("for shaking out a flaky test") plus `--seed` for order randomisation. All `make test` added
was incidental CPU contention from 928 parallel Rust tests. The right answer was never a
different runner — it was a test that does not depend on load at all.

## 2026-08-01 (cont.) — tooling bundle 1: REPL command registry + debugger path-B locals capture

Two shipped, from the "finish off tooling & telemetry" sweep.

**REPL command registry.** `std/tool/repl.blsp`'s `,cmd` meta-commands were a hardcoded
name-list (`*repl-meta-names*`) + a dispatch `cond` + per-command handlers — three coupled
places to touch per command, no extension seam. Replaced with a registry: `*repl-commands*`
(an ordered list of `{:names :usage :summary :handler :hidden}`), `register-repl-command` as
the public seam, and `,help` generated from it. The built-ins re-register themselves at module
load. `tests/repl_test.blsp` gains a registry `describe` (aliases, dispatch, replace-on-reregister,
`:hidden`).

**Debugger path-B automatic locals capture (the ROADMAP item).** `eval-at` used to see only the
values you named at `break`; now it sees EVERY in-scope local. The mechanism is a new compiler
intrinsic `(%scope)` / `(%locals)`: the VM compiles either 0-arg call straight into a fresh
`{:name → value}` map from the compile-time lexical-scope table (`compile_scope_map`,
`eval/compile/mod.rs`), keyed by the name as a **keyword** (same interned `Symbol`, so `%eval-in`
binds it) so a named `:val` overrides a captured local of the same name on `merge`. The
tree-walker keeps the env-frame builtin as the fallback (also switched to keyword keys, for
engine parity). `break`/`break-when` became **macros** so `(%scope)` expands in the caller's
lexical scope — the snapshot now carries `:scope` (auto-captured) + `:vals` (explicit, wins).
One edge: `(%scope)` as a defn's *sole body* hits the pre-existing passthrough-alias
optimization and resolves to the builtin — harmless, since the debugger always uses it as a
subform. Verified across VM / tree-walker / no-JIT / GC-stress. `tests/debug_test.blsp` gains a
path-B block (locals not named at break, computed exprs, `:val` override, `break-when`).

**`,resume` / `,step` under `pry`.** `pry` binds a `*debug-session*` dynamic and registers
`,resume [N]` (resume all / the Nth parked) + `,step` (advance one) through the new registry, so
a debug session is driven with `,`-commands instead of full `(debug/…)` forms.

## 2026-08-01 (cont.) — thread 6 narrowed: three mechanisms excluded, still not the supervisor's

Kept after the JIT closure re-promotion. No fix, but the search space is much smaller and the
exclusions are worth more than another guess.

Direct probes, each measuring promotions per operation against the supervisor path's **0.6/op**:

| shape | ops | promotions | verdict |
|---|---|---|---|
| create a capture-free closure in a hot arm | 200 k | 1 | clean under JIT, VM and tree-walker |
| create one and **send** it | 100 k | 2 | clean; `BROOD_NO_SHARE_FN=1` changes nothing |
| **receive** a thunk, **call** it, spawn inside it | 20 k | 17 (JIT) vs 5 (VM) | ratio present, absolute negligible |

So ADR-194's share path is fine, `MakeClosure` in a hot JIT'd arm is fine, and even the
receive-then-call-then-spawn shape — which is what I assumed the supervisor reduced to — is
three orders of magnitude too small. Whatever `start-child` does beyond that is the culprit:
it also `link`s the child, optionally `register`s a name, and `assoc`s the spec into the
supervisor's long-lived state map.

Recording the exclusions in the handoff so the next attempt bisects
`supervisor--start-child` itself instead of re-probing generic shapes. The standing suspicion
is unchanged: `make_closure_cached` caches only when `fn_rest` is a RUNTIME pair and bails
**silently** otherwise, which is exactly the shape of a JIT/VM divergence that costs 2×.

## 2026-08-01 (cont.) — thread 6's cause found: a `spawn` thunk re-promoted per call

Found it, with a tool rather than more bisecting. **`spawn`/`spawn-link` must promote the
spawned thunk into the shared RUNTIME region** — a new process cannot run code out of another
process's LOCAL heap. At ordinary call sites that is free after the first time, because the
compiler **const-lifts** a capture-free thunk (`eval::compile::const_node`): promoted once,
reused forever. On the supervisor path it is *not* const-lifted, so the thunk is rebuilt LOCAL
on every call and promoted again — into a region that only ever grows.

`BROOD_TRACE_PROMOTE=1` on a supervisor workload, ranked: **1382 of 1389 promotions from a
single site, `spawn_impl <- spawn_link`.** The remaining handful are the compiler's own
one-time const lifts. Hours of elimination bisecting had produced less than that one run.

**What that trace cost to build: about ten lines.** I should have written it far earlier. The
whole day's method on this thread was excluding mechanisms one at a time, and I excluded nine
of them correctly — creating a capture-free closure in a hot arm; creating and **sending** one;
receive-thunk + call + spawn; `spawn` *and* `spawn-link` in a hot loop from the root *and* from
a spawned process; `link`; storing the spec in long-lived state; the restart window; and the
reclamation policy (`BROOD_RT_GC_FLOOR` at 64 / default / 100000000 → 2512 / 2663 / 2661, so no
feedback loop through compaction). Every one of those was true and none of them found the
cause. Ask the runtime what it is doing before deducing what it must be doing.

One result along the way is worth keeping because it killed an attractive theory: the closure
**form is irrelevant**. `:start` written as an inline anonymous closure and as a plain global
function reference promote 11 876 vs 11 877 per 20 k. It was never about anonymous closures;
it is about whether the thunk got const-lifted.

**Not fixed, and this one is a genuine design call.** Two shapes: teach the compiler to
const-lift the thunk on this path too, or memoise the promoted form at the spawn boundary the
way `store_const_closure` already does for const closures. Both sit on the scheduler/GC
boundary — the part of this system with the worst history — and today has already produced two
reverted "fixes" that looked right in isolation. It wants a deliberate pass.

**`BROOD_TRACE_PROMOTE=1` is kept and documented** in CLAUDE.md's debug table: it names every
closure entering the append-only region along with the Rust frames that put it there. Anything
promoted *per operation* is an unbounded leak of shared code whose symptom is a slow decay
rather than a crash, so this is the first thing to reach for next time.
