# Dev log

Chronological record of work sessions. Newest at the bottom.

## How to navigate

The session history is split so this file stays loadable:
- **This file** = the digest (one line per session, by date) plus the **most recent days
  in full** at the bottom, where new entries get appended. The load-bearing *why* of any
  change lives in its `## ADR-NNN` ([decisions.md](decisions.md)) or topic doc, not in a
  blow-by-blow session log.
- **Older full entries are in [`devlog/`](devlog/), one file per month** —
  [`2026-08`](devlog/2026-08.md), [`2026-09`](devlog/2026-09.md),
  [`2026-10`](devlog/2026-10.md) — oldest first, each opening with a list of its entries. They
  were moved there unedited on 2026-10-15; the digest below stops short of many of them, so
  for August onward a month file's list is the complete timeline.
- The full verbatim text of the **early** sessions (through 2026-05) lived in
  `docs/archive/devlog-archive.md`, deleted 2026-10-09; git history keeps it.

You rarely read this top to bottom. For the *current* state of something, prefer
the topic doc (see [../README.md](../README.md) § Documentation) or the relevant `## ADR-NNN` in
[decisions.md](decisions.md). Use the digest to place a change in time.

Entries are historical and are not rewritten, so a few older ones link to design
docs since deleted (`concurrency-v2.md`, `supervision.md`, `incremental-check.md` —
mostly trimmed in `fdce540` once their features shipped). Recover from git if needed.

**Maintenance:** keep this lean. Append a new session as a **full entry** under
"Recent"; once it's older than a day or two, give it its **one-line digest entry** and
move the full text to its month's file in `devlog/` (unedited, appended in date order, with
a line in that file's entry list). Prune anything that won't help future work — the ADRs
and topic docs carry the durable rationale.

**The digest is the invariant**: every session gets its one line, even when the full entry
stays in "Recent" — the digest is what the rest of the file promises.

**Major threads** (grep these to follow an arc end to end):
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

Every session, oldest first; the latest days are in "Recent" below.

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
- **2026-08-13** — ADR-223 Phase 2 (MVP): cross-file require-by-name for a co-located module
- **2026-08-13** — closing out: the whole-tree format, and KI-39's local avenue exhausted
- **2026-08-13** — `nest check` (and `brood`) no longer SIGABRT on a broken pipe
- **2026-08-13** — the VM's call path was contending on one cache line (ADR-224), and the gates that should have caught it
- **2026-08-13** — `reduce-while`: the early-terminating fold (roadmap: Elixir-loved ergonomics)
- **2026-08-13** — require-by-name for a co-located module in a nameless project / bare run (ADR-223 Phase 2b)
- **2026-08-13** — Function-head guards: `:when` on defn/fn clause heads (ADR-226)
- **2026-08-13** — `tap` / `then`: the single-function pipe helpers (Elixir Kernel parity)
- **2026-08-13** — Stdlib namespacing, stage 1: the `enum` sequence-helper module (ADR-227)
- **2026-08-14** — Stdlib namespacing, stage 2: the `map` transformation-helper module (ADR-227)
- **2026-08-14** — Stdlib namespacing, stage 3: the `math` library module (ADR-227)
- **2026-08-14** — Auto-derived imports go live + stdlib namespacing stage 4: `json` (ADR-227 follow-up)
- **2026-08-14** — The computed-head call pays no allocation: the arm handle is memoized (ADR-228)
- **2026-08-14** — Remove the user-facing `require` form; loading is inference + an internal `require-one`
- **2026-08-17** — The stale-bare-name sweep: a curated sig masking the unbound lint, two dead benchmark rows, and one GC walk
- **2026-08-17** — `nest release` bundle rooting: dev/release parity for package-rooted deps (ADR-070)
- **2026-08-17** — Close the last two open issues: the `sqrt` call-site inline (KI-44) and the stale `examples/editor` (KI-45)
- **2026-08-18** — KI-39 was never a flake: a coloured log, an uncovered TIMEOUT, and a keypress that loaded 2967 lines
- **2026-08-18** — Stdlib namespacing, stage 5: the `string` library module (ADR-230)
- **2026-08-18** — Stdlib namespacing, stage 6: the `file` library module (ADR-231)
- **2026-08-19** — Bump to 0.4.0: the stdlib-namespacing renames are a compatibility boundary
- **2026-08-19** — The dependency resolver is now brood-version aware (ADR-209)
- **2026-08-19** — Release bundles: root a dependency module only when its name collides
- **2026-08-19** — KI-36 reproduced and fixed: the deadline analysis was aimed at the wrong branch
- **2026-08-19** — Stdlib namespacing, stage 7: flat names for `proc/*` and `net/*` (ADR-233)
- **2026-08-19** — Stdlib placement review: path/file/hash/crypto homes + enum→seq (ADR-234)
- **2026-08-19** — `(:use …)` clashes resolve lazily; the maintained overlap list is gone (ADR-235)
- **2026-08-19** — Stdlib prefix cleanup, exemplar: `queue`/`pq` (ADR-236)
- **2026-08-19** — Prelude split into std/prelude/*.blsp (organizational)
- **2026-08-19** — Prefix rollout, long tail: io/task/reload/protocol/gen/hash/format/package/telemetry/explain (ADR-236)
- **2026-08-19** — Forced through the collision keeps: stats, fuzzy, tcp, sse (ADR-236)
- **2026-08-19** — Going-live namespace policy recorded (ADR-237)
- **2026-08-19** — Release 0.5.0: the stdlib-naming compatibility boundary
- **2026-08-19** — Green the tree after the ADR-236 sweep: fixture damage, a prelude-split guard, a pq bench
- **2026-08-20** — A direct git/path pin overrides a transitive registry dep (ADR-238)
- **2026-08-20** — The rename-sweep gap, closed: a gate for `stress/` + `scripts/fuzz/stress/`
- **2026-08-20** — KI-47's memory question, answered: legitimate growth, and the mechanism was wrong
- **2026-08-20** — The source-position tables are not the memory target: 7.1%, not 18% (a negative result)
- **2026-08-20** — One conversion-naming convention: arrow `->` everywhere (ADR-239)
- **2026-08-20** — Kernel primitives stay flat (dash); a `/` is module-member syntax only
- **2026-08-20** — Refreshing the perf standing: the menu was being ordered off a superseded table
- **2026-08-20** — The "no default features" CI gate has never worked (and my own break proved it)
- **2026-08-24** — a primitive's name gets one definition site; CST-backed `nest rename` (ADR-240)
- **2026-08-24** — a library name may shadow core only when used qualified (ADR-241)
- **2026-08-24** — the bare core: 613 published names down to 337 (ADR-242)
- **2026-08-24** — the namespace waves: 337 bare core names down to 291
- **2026-08-25** — `brood --check` resolved `(:use M)` against the namespace, not the module
- **2026-08-25** — KI-55: a shipped closure now brings its modules with it
- **2026-08-25** — ADR-243: a framework's client API is a module, not ten bare names
- **2026-08-25** — ADR-244: a late reply now dies at the door, not in the mailbox
- **2026-08-25** — KI-56: bounding the L1 copy, and why counting nodes was not a bound
- **2026-08-25** — the overflow check that overflowed, and 17 guards CI never ran
- **2026-08-25** — KI-56's second site: the fix that turned out not to be a trade-off
- **2026-08-25** — KI-57: the suite was 1061/1061 green with a use-after-GC in it
- **2026-08-25** — the first cross-language run on 0.11.0, and the inline a rename had retired
- **2026-08-25** — KI-59: the CI-friendly run mode reported failure for programs that worked
- **2026-08-25** — the gate KI-58 prompted found a second dead inline on its first run
- **2026-08-25** — the stdlib had lost stderr, and only a checker test noticed
- **2026-08-25** — the same one-line fix, arrived at twice
- **2026-08-25** — the startup image did not work on the build that ships
- **2026-08-25** — the benchmark "regressions" were mostly measurement, and one of them was real
- **2026-08-25** — correcting KI-63, and the measurement method that manufactured a threshold
- **2026-08-25** — KI-63 retracted: three methods, three confident numbers, no effect
- **2026-08-25** — the stdimage boot-install premise, re-measured rather than inherited
- **2026-08-25** — why every probe said the image was fine while the suite said 150 failures
- **2026-08-26** — KI-61 fixed: the prelude autoloads instead of force-loading; boot 22.8 -> 11.6 ms (ADR-246/247)
- **2026-08-26** — KI-64 fixed: a JIT block-argument spill was landing on the deopt journal (ADR-248)
- **2026-08-26** — the codecs: `json` parse 1.8x (row -20.8%) and `base64` decode 1.8x (row -9.5%) (ADR-249)
- **2026-08-26** — every package's `:brood` floor was a lie; the ecosystem release train that fixed it
- **2026-08-27** — migrating the ecosystem across the waves; two outages, both from verifying the wrong artifact (KI-66/67)
- **2026-08-27** — stdlib cleanup: `name` folds into `->string` (ADR-258); the seq orphans go home; `last-index-of` moves to `string/`
- **2026-08-28** — the capture hazard behind that rename: prelude templates, and finishing the `/name` root escape (KI-73)
- **2026-08-27** — the type system, audited then rebuilt: `sig` fails closed and the definition owns the arity (ADR-259), the walk's totality is gated and found the quasiquote gap (ADR-260), a parameter's type is its domain (ADR-261), a union keeps its terms (ADR-262), `(not T)` (ADR-263)
- **2026-08-28** — a record is closed, and openness is the type of the keys it doesn't declare (ADR-264) — which is what makes ADR-262's tagged union usable rather than merely representable
- **2026-08-29** — arrow decomposition (ADR-292)
- **2026-08-29** — `BROOD_MONO` had never been run: Tier 1 was miscompiling, and now proves the identity rather than the impl (ADR-294)
- **2026-08-29** — ability-op runtime contracts (ADR-293), and the discovery that `BROOD_CONTRACTS=1` had been unusable on every cold boot cache — three defects, no end-to-end test (KI-81): an intersection of arrows satisfies what no single arm does, checked against a brute-force model of what an arrow denotes rather than against more property laws
- **2026-08-29** — KI-87: the inference cycle guard released the symbol it refused (`bool::then_some` builds its argument eagerly, so a refused `InferGuard` was built, dropped, and un-marked the in-flight inference) — `nest run` at 54 GB, three 19 GB test processes; one-line fix, sabotage-verified guards, `ulimit -v` in front of every inference test run
- **2026-08-30** — strict over std 336 → 0 and a CI gate for it: ~350 sigs declared by reading bodies, every nil source made honest (one real bug), and FOURTEEN checker gaps closed generally on the way (extremum/get/nth defaults, short-circuit-exact `or`, branch narrowing in inferred returns, record names carrying field types, optional defaults, destructuring, dead branches, exclusion-known negations, fold fixpoint, prelude sigs surviving the freeze, sigs inside `check-allow`)
- **2026-08-30** — type-guard signatures (ADR-301): `(sig datetime? (any -> (is datetime)))` narrows like a built-in predicate, bare-local or path, cross-module; the prelude's six record predicates declared
- **2026-08-30** — mailbox bounds (ADR-308): `(proc/flag :max-mailbox n)` — senders check at enqueue but are never blocked and drop nothing; the flooded process raises catchable E0046 at its next safepoint or receive (the `:max-heap` protocol on the queue axis)
- **2026-08-30** — `try … finally` (ADR-306), a prelude macro clause over two `%try`s; and `throw` of a caught error map rebuilds the error, so a rethrow (and everything escaping a `finally`) renders as the original, not as a map dump
- **2026-08-30** — crash reports by default (ADR-305): `proc/system-monitor` becomes one subscription per pid with an `:exit-abnormal` selector; `std/proc/crash-report.blsp` prints each crash site once with the trace; armed by `brood file`/`nest run`/bundle/REPL, never `nest test`; the kernel's `process N died` one-liner yields to it
- **2026-08-30** — Lisp survey: Brood vs Clojure/CL/Racket and vs OTP; two backlogs recorded in ROADMAP ("Lisp survey + OTP-gap backlog") — seven OTP-gap items (crash reports by default, `defapp`, `finally`, `defstatem`, registry/`pg`, mailbox bounds, soak) and ten Lisp borrowings (regex captures, `iterate` as a seqview, sorted map/set, `reduced`, macro grammars, `next-impl`, `for :into`, small helpers, contract blame); the `terminate/2`-on-kill item reassessed as by-design
- **2026-08-31** — `gui/font`'s one-arg form never worked (&optional nil-padding)
- **2026-08-31** — `scan-form-start` tracks bracket depth, not column 0
- **2026-08-31** — the block cursor gets a rim
- **2026-08-31** — the stability/perf audit: four silent bugs fixed, the rest filed
- **2026-08-31** — the attach seed could clobber a first keystroke (collab)
- **2026-08-31** — `spawn-monitor`, and the two gates that let a release ship unbuildable
- **2026-08-31** — a bare `(fn)` panicked the recursion analyzer
- **2026-08-31** — the checker learns the shape of the migration's bugs
- **2026-08-31** — inline pattern-clause callbacks stop reading as `any`
- **2026-08-31** — ADR-310: a known failure is a value, not a raise
- **2026-09-01** — the benchmark refresh found a regression the whole gate stack was blind to
- **2026-09-01** — a failure is truthy: the migration that cost nothing was the tell
- **2026-09-02** — the type said `any` because the program was wrong
- **2026-09-02** — a CI flake that was a real bug: `tcp/set-binary` could not make a server binary
- **2026-09-03** — (types) the standard library was telling the checker to forget
- **2026-09-03** — (correction) there was no flake; I read a cancelled run's log
- **2026-09-03** — (stability) a watchdog for the process that never ran, and a rule for the env var that leaked twice
- **2026-09-03** — (runtime) `read-line` parks a process, not a worker: ADR-059 Phase 2, and KI-97 closes
- **2026-09-03** — (CI) the strict gate was red all day, and the downstream smoke told two other stories
- **2026-09-03** — (stability) the non-reproducible entries, one by one: two mechanisms were wrong, one was three bugs, one stays dormant
- **2026-09-04** — the tree-walker→VM router is default-on (ADR-318); KI-88 archived as dormant
- **2026-09-05** — KI-109 measured: where the 3% lives, three levers priced, two leads
- **2026-09-06** — the two loose ends: a deprecation the editor could not show, and a scaffold that warned on its first command
- **2026-09-07** — heap.rs split, second move: the env chain and the global table
- **2026-09-07** — KI-114: a float-profiled arm applied to an int published a float
- **2026-09-08** — KI-117: a JIT'd error has no `:trace`, found as a one-in-N flake
- **2026-09-09** — two sessions split `heap.rs` in parallel; reconciling it, and `RuntimeCode` moves out
- **2026-09-09** — an idle runtime burned 6-8% of a core: every parked worker woke 100x/s forever
- **2026-09-09** — a gate for ADR-323's third spelling; the `nest` arms item was already done
- **2026-09-09** — KI-121 fixed without waiting for it to recur: the test, not the reporter, was racy
- **2026-09-09** — queue items 2/3/4 closed by verification; a doc slice, and why its sabotage lied
- **2026-09-09** — the wasm cooperative scheduler was compiled on every CI run and executed on none
- **2026-09-09** — the bare namespace gets a gate: 264 names recorded, a new one fails by name
- **2026-09-09** — KI-122: the KI-120 tripwire was crying wolf on bedit, on a false claim
- **2026-09-12** — `filter` joins its complement in `seq/`; `remove` becomes `reject` (ADR-330), and docstring `[[links]]` finally render (ADR-331)
- **2026-09-12** — pre-compilation, counted: bytecode is 2.5 ms of `json`, Cranelift is 60–80 ms, and both persistence ideas are dead (compute-frontier §7.11)
- **2026-09-12** — the tier threshold is a call threshold and it is 128 (ADR-333): boot stopped queueing 139 compiles ahead of the hot arm — `spawn` −31%, `fib` −18%, `bintree` −14%
- **2026-09-12** — the refused arms are the hot core (108k activations on json, dispatch 43% of its main thread); leaf admission built and not landed; the lever is the native→native call (compute-frontier §7.12)
- **2026-09-12** — KI-133: a preempted native loop resumed on the interpreter via its callee's frame — 5M-call loop 3.86 → 2.53 G instructions, the leaf-spliced one −72%
- **2026-09-12** — the inline xcall blob was refused at native depth 0 (the frame every main loop lives in); allowed, and the per-call stack re-stamp dropped — 234 instructions per native→native call, from 640
- **2026-09-12** — every native arm fetched its roots base through an FFI at entry and after each call; now one load — 218 instructions per native→native call
- **2026-09-12** — the GUI retains its frame and repaints by cell row (ADR-332): a keystroke paints 0.3 ms instead of 4–13; whole-pixel ppem, subpixel text at 1× (`gui-text-aa!`), snapped hairlines, `gui-line-height!`
- **2026-09-12** — `0xFF` reads: radix literals, the second ADR-169 reservation to pay out (ADR-334)
- **2026-09-12** — a qualified reference loads its module on first use (ADR-335): `nest complete` 72 → 20 ms, five modules instead of 62
- **2026-09-12** — the lazy-load wave's two artifact divergences: `REGISTRIES` closed (`bd88a386`), `FACTS`/`GLOBALS` shown to be the lazy-load win and normalised in the gate
- **2026-09-12** — KI-131: lazy loading met `%isolate`; the runner declares its closure with a new `(:load …)` header clause
- **2026-09-13** — the VM→native direct call lands as the frame path minus the round trip (`mandelbrot` −12%, `supervisor` −5.4%); on the way in: the fast-link probe handed back native flat cells as Brood links (the §7.12 "runaway"), and a nested tail chain put 19 820 dirty parks on `supervisor`
- **2026-09-13** — the timer thread is woken only for a new earliest deadline: a parked `(receive … (after ms …))` no longer costs a futex wake per park (2.5 → 2.0 µs on the timed round trip)
- **2026-09-13** — KI-134 FIXED (ADR-339), two mechanisms: the isolate rollback (journalled loads, replayed atomically; `%isolate-discard-loads` for the image builder) and — traced, not the rollback — the image branch binding a module before its impls, reached through a lazy global hit (publication order fixed). The source-path residue is KI-135
- **2026-09-14** — `nest check` is order-independent (KI-137 FIXED as a gate): every module the listed files require or name in a body is loaded, sorted, then materialised to a fixpoint, BEFORE the first check (`project-preload!`, `%check-materialise-referenced!`); a verdict is a function of the list. Verified with the `max`/`min` declarations removed — four list orders that split 0/1 now agree — and guarded by `tests/check_preload_test.blsp` (two fixture modules record at load whether the other is provided; listed in reverse, the sorted order still shows; sabotage-verified). A file's own module is not loaded unless its body names it, so checking a test file still runs none of its top level
- **2026-09-13** — the afternoon's reds, cleaned: `buffer-registry` replied to a caller BEFORE notifying the watchers, so a caller that was also a watcher could `drain` a notice that had not arrived (the test's "round-trips are barriers" premise; now notify, then reply), and its kill-then-reshare test raced `exit`'s next-tick delivery (it now waits for the death, bounded — the registry's `[:down]` may still be queued, which is the case under test); the seven `editor/configs` / `editor/lexer` span functions carry executed examples (audit ratchet back at 1138); `editor/lexer` `(:load editor/highlight)`s — from source every `:syntax/*` face resolved to nil (`configs_test` 2/10) and only the image's over-attributed `*faces*` replay made it pass, filed as KI-136. The `math/clamp` strict red seen twice (merged tree, and a clean origin build) then not in seven runs turned out to depend on whether `datetime` was LOADED when `math.blsp` was checked (`%max`'s number-or-comparable-record contract; the record half is inhabited only after a module registers comparable records) — a verdict that varies with `nest check`'s parallel load order, filed as KI-137; `math/max` and `math/min` declare `(& ?A -> ?A)`, sound once the checker lets a module's own qualified definition read its by-name rule and a type variable absorb an untyped argument (881552a6, 6367ac6e)
- **2026-09-13** — the checker's 6.7× slowdown (ADR-340/341) attributed and mostly recovered: `reflect/check-file` over all of `std/` went 15.6 s → 104.6 s at `e64b65f8`, and `std_check_test` sat at 105 s against the 120 s per-unit cap. Not the fixpoints' arithmetic — a `perf` profile read 70% in `Vec<Ty>::clone`, `RawTable<(Symbol, Sig)>::clone`, `Ty::drop` and the allocator: every `Ctx::clone` (each `bind`/`narrow`/branch, thousands per walk) deep-copied the ~20 FILE-level maps, and the two nested fixpoints turned one walk of the file into up to a few dozen. `Ctx` is now a per-scope half plus an `Arc<FileFacts>` shared by reference (`Arc::make_mut` for the between-pass mutators — value semantics unchanged): 104.6 → 31.4 s. The live set is walked once per file instead of once per joint round, and a joint round whose returns did not move reuses its derivation instead of re-walking the file to reproduce it: → 28.2 s. `std_check_test` is one unit per std file (parallel, own cap, fails by file name): 105 → 18.5 s wall. Checker output identical: `nest check` std+tests+examples and strict std at zero both before and after
- **2026-09-13** — the pre-push hook grows the fast half of CI (`make prepush`): the `BROOD_*` flag catalogue as a grep, and with a `.blsp` involved the tests naming the changed modules, the surface audit and the executed doc examples — the three reds that landed on `main` this afternoon (`BROOD_GUI_BLIT` uncatalogued, nine public functions without a `form → result` example, `sig_adoption_test` reading ADR-341's old tuple spelling) each fail it in seconds; all three fixed, sabotage-verified both ways
- **2026-09-13** — KI-135 FIXED (ADR-344): a module publishes whole — every load stages its defines and registry OPERATIONS in a per-load frame the loader reads before the table, and installs them under one write of the globals table when it completes (discarded when it throws). The whole-map `%swap-registry!` write-back cannot be staged (an outer frame's copy published over a nested load's additions and cost the image builder 10 of 35 require-edge records); it writes live unless the registry was born in the same load. Guard: a 300 ms-wide fixture window, 0 sightings vs 58 bypassed
- **2026-09-12** — memoised view fragments (ADR-336): `ui-memo`, the frame carries its cache back; `=` is O(1) on the same cell, `append` shares its last list; `BROOD_UI_TRACE`; KI-132 filed
- **2026-09-12** — text contrast is a setting (ADR-337): `gui/text-contrast` lifts light-on-dark stems under the linear-light blend; bedit ships 1.4
- **2026-09-12** — `editor/shell`: shell-script highlighting, and a script typed by its `#!` line (`register-interpreter-type`, Emacs `interpreter-mode-alist`)
- **2026-09-12** — `\b` / `\B` in the regex engine: the Pike VM answers them, and a boundary pattern's `match?`/`matches?` route there too
- **2026-09-13** — Caller-derived parameter types for EVERY single-arm function a file defines (ADR-341, second cut): the derived type is a fact about the file's calls, not about privacy — a body warning says "every call in this file would fail here", the sharpened return is read by this file's callers only. Two general rules it needed: `check_let`'s per-binding scope is one function (`let_bind_scope`) both walkers call, so an `and` temporary's guard narrows at a call site; a falsy `or` refutes every biconditional disjunct in the else branch (`or_disjunct_guards`), each on its own variable
- **2026-09-14** — two types:: reds behind `(sig max (& ?A -> ?A))` (0fcbdee5), attributed and fixed: a type variable unified against an argument the checker cannot type now absorbs `any` (`(math/min 3 x)` read exactly `3`), and the curated `math/min`/`math/max` entries are gone — the declaration is the contract, and beside it they only tripped the widening gate
- **2026-09-14** — Recursive types (ADR-349): `(rec X …)` in the lattice — a μ binder and a self-reference, every relation unrolls it coinductively, the inference fixpoints fold an ascent that nests itself into one and confirm the fold on the next round; a JSON-shaped decoder reads `(rec X 1 | nil | vector<X>)` where the depth cut read `vector<… | vector<any>>`
- **2026-09-15** — `:pure` and `:total` on a `sig` (ADR-351): `(sig f (int -> int) :pure :total)`, or `(sig f :total)` alone, registered beside the type. `:pure` walks the body for an effectful head through the functions it calls (a deny-list); `:total` asks every self-call for a structural decrease read in its branch scope (a shorter non-empty list, a smaller bounded-below int, a larger int under a literal or a count bound) and the walk reports a `match` failure it cannot prove unreachable. `ui-memo`'s thunk is checked with no declaration — and found `tests/ui_test.blsp`'s deliberate one. `path/join` and the regex DFA loops carry the first declarations; a count relation between two PARAMETERS is now derived from the callers (ADR-350), which retired `std/regex`'s last two `check-allow :type-mismatch`; a `& rest` binder is `nil | list<rest>`
- **2026-09-14** — Lengths and indices (ADR-350): an interval on `int` and a length on every countable, hulled on union, met on intersection, widened to infinity in every fixpoint; `count` reads it, the builders move it, `empty?`/a comparison/`(= (nth a k) lit)` narrow it — `(nth words 1)` under `(>= n 4)` is a string, and a `[:ok x] | [:error msg]` dispatch types the whole value. `(list E)` is now `nil | list<E>`. On the way: a sealed op's domain only on the op itself (`tempo/->iso`), a recursive specialization typing its self-calls in their `if` branch (`path/join`), the byte encoders' octet contract in their sigs. std 0/0, tests plain 0
- **2026-09-14** — A list has a positional shape (ADR-348): `(list a b)`, a quoted list, `cons`/`rest` and a `& rest` binder carry `(list T U)`, read by `first`/`nth`/`count`/destructuring/the contract — the `pair | map` at `(first (list m …))` is gone; over the node cap a shape degrades to its element union, not bare tags
- **2026-09-13** — A module's own qualified definition of a name the by-name rules describe IS that function: `(math/max lo (math/min hi x))` inside `std/math.blsp` reads the extremum rule (its operands' union) ahead of `max`'s inferred return — which was the registry-derived `ordered` cover, so `clamp` was strict-clean checked alone and red with the project loaded (the pre-push hook's `math/clamp`)
- **2026-09-13** — A handover is a site (ADR-341, third cut): a candidate handed to `map`/`fold`/a callee with an arrow derives its parameters from what the callee promises to call it with, through the one `callback_seed` the walk seeds a `fn` literal from; only a promise-less use (`apply`, a value in a map) still escapes. And an arrow in HEAD position describes the call (ADR-347): `((cur 1) "x")` types as the arrow's result, exact arity, arguments checked by the one per-argument rule
- **2026-09-13** — Caller-derived parameter types for private functions (ADR-341): a `defn-`'s parameters are the union of what its file's call sites pass, a least fixpoint jointly with the private returns; `std/json.blsp` is strict-zero with no signature on its ten-level parser chain; a call with an uninhabited argument is ⊥; tuples merge by position; `Ty::widened_below` for the round a recursive value shape would nest forever
- **2026-09-13** — Declare at the leaf, derive from the call (ADR-340): a self-recursive function is specialized at its call by a joint fixpoint (`(sum-to 10 0)` is `int`, not `any`); the loaded-closure "Tier 1" that lost nested demands cross-module is gone; the checker materialises what a loaded body names so `text/char->line`'s leaf sig reaches `buffer-current-line`; a `let`-bound lambda checks its calls; a private constant has its value; `nil | list<3>` ∪ `list<int>` is one term; a dynamic union is read term by term
- **2026-09-13** — `:on-move` is a layer facet (ADR-338): a follower runs only when point or the text moved (`buffer-moved?`), never on every event like a `:post-key` guard; `string/width->index`, the inverse of `display-width`, for a click on a wide glyph
- **2026-09-13** — the checker types a `def-` literal like a `def` one: Gap A reads `top_level_defs`, so a private `(def- k 10)` is an `int`, not `dynamic()` — bedit's strict count 13 → 0
- **2026-09-13** — `--version` says `-dirty`: a binary whose `crates/`/`std/` differed from its commit is no longer indistinguishable from a clean build of the same sha; `make doctor` names it; build.rs re-runs from a worktree too
- **2026-09-13** — tabs are a column-dependent cluster (ADR-342): `display-width` / `width->index` take `start-col` + `tab-width`, `string/expand-tabs` is the third leg, the GUI paints a raw tab to the screen stop; the scroll blit (ADR-343): a dirty strip that is a translation of old canvas rows is copied, not drawn — a 1080p scroll paints in 2.3 ms, not 10; `ui-coalesce-motion` collapses a `:move` flood like a `:drag` one; `:close-is-input?` lets an app ask before the X button quits; `buffer-file-changed?` — a buffer stamps its file's mtime at read/save
- **2026-09-13** — `editor/buffer-registry` (ADR-345): a named directory of buffer processes — share-once, enumeration, membership notifications — the seam a second frame (a process with its own window) joins the same buffers through
- **2026-09-13** — `editor/lexer` (ADR-346): a lexical language mode is a table — structured rules, word classes, line rules — and `editor/configs` is JSON / YAML / TOML / Makefile / INI / commit-message as tables over it
- **2026-09-15** — the four open known issues closed: `apply` binds a callee's type variable (KI-140); the dense table region is chunked, 6.4 GB → 52 MB reserved on `regex_test` (KI-142, ADR-354); registry writes are attributed to the module whose load made them and the image differential compares per-module registrations (KI-136)
- **2026-09-16** — v0.29.0 (the open issues closed) and v0.29.1 (KI-147: a refer-all mistook another process's finishing load for a cycle, so `nest release` refused every hive bundle since v0.28.0); every project at `:brood ">= 0.29.0"`; hive deployed on v0.29.1
- **2026-09-15** — ADR-355: a `let`-bound `fn` literal's parameters are derived from its callers (the ADR-341 rule scoped to the binding; sites under callback literals and nested `let`s type), and a `sig` naming the required positions seeds a `defn` with undeclared `&optional`s. bedit strict 58 → 48 with no bedit change; downstream sweep of 16 projects green (hive's reds = version skew)
- **2026-09-16** — v0.29.2 (ADR-356: `markdown/->html` a core module — hive's lean bundle could not find `docs`); hive's tests render the running version's reference seeds; then bedit strict 48 → 0 through three checker rules (`get-in` over a literal path reads the declared shape, `%map-pairs` walks a `map<K, V>` as its entries, the site collector walks a `let`-bound lambda under its derived parameters) and bedit's own leaves; then three more derivation gaps (an `&optional` function.s collected parameters, a let-bound lambda.s self-call folding to ⊥, a fold accumulator.s fields through the ascent) and the four workaround `sig`s deleted; then 323 redundant `sig`s removed from std (`scripts/redundant-sigs.blsp`), five kept because a neighbour or a caller read through them; KI-149 fixed (the whole-project lints ran only in the bare `nest check` and never counted — both forms run and count them now)
- **2026-09-16** — ADR-360: the linear-map rewrite recognises the tally a user writes (`(assoc m k (+ (get m k 0) e))`) — `wordcount` 857 → 66 ms, `persistent-map` 535 → 79 on the idiomatic ports; `%table-add` is the fused op and `%map-int-add` is now exactly the same `+` (KI-151); a linmap fuzzer with the oracle in the program.
- **2026-09-17** — KI-152: the linear-map rewrite was observable on its seed (a rope in the input map raised on one arm, returned on the other); `%table-from-map` declines and the split keeps the loop as written behind the seed check, under a new `(check-allow :generated …)` the checker skips — measured free. Go column published in brood-benchmarks (1.8× C, between C and .NET).
- **2026-09-17** — KI-153: a function whose only sites were its own self-calls derived ⊥ for every parameter and was checked as dead code (no lint in its body at all); a self-call no longer makes a function live. The rewrite's forms keep their source positions.
- **2026-09-17** — `inc`, `dec` and `(- (get m k 0) e)` fuse like `+` (ADR-360 §5, `%table-sub`); the fuzzer draws the four spellings and an i64::MIN seed.
- **2026-09-17** — ADR-360 §6: the tally through a `fold`/`reduce` LITERAL builds in place too (957 → 98 ms on 750k keys), and `seq/frequencies` is written as that idiom (523 → 96 ms); a shadowed name anywhere in the form declines the rewrite; the fuzzer gained a fold arm against a named-function reference.
- **2026-09-17** — call convention rung A4, first half: the callee nils its own locals (unrolled, before the stack guard) and the inline call's fill loop is gone — 380 → 321 instructions per call on a six-local callee, `bintree` −4.5%, everything else inside its floor.
- **2026-09-17** — KI-156: no `letrec` loop in the language had its `SelfCall` — the self-name's own capture slot read as a shadowing local — 300 → 4–13 ms on a 3M-iteration local loop; every named local loop, `defseq`'s, and the `for` macro's pipeline were running through a full dispatch per iteration.
- **2026-09-17** — KI-157: a native loop never saw a pending memory limit (E0043, the process heap limit, a mailbox overflow are VM-safepoint checks) — the back-edge poll now reports one and the loop deopts to raise it; found the moment letrec loops went native.
- **2026-09-17** — ADR-360 §7: `seq/l*` pipelines fuse into one literal, a fold over a range is a counted letrec loop, a passthrough reducer takes the HOF fast path — `pipeline` −49% (3 735 → 460 instructions per element), `(fold (range …) 0 (fn …))` 410 → 21 ms; the checker reads the author's code, never the rewrites.
- **2026-09-17** — `tests/` to zero strict and a gate (from 32): `math/pow` had declared nothing for nineteen days (its `sig` sat inside its docstring — `sig_placement.rs` now reads every docstring); a declared overload's matching arms MEET (ADR-116 addendum); an assertion's `(pr-str (quote …))` is not an escape, so functions under test are derived; `record?` is a one-sided guard; `range`/`into`/`map` keep their lengths, `bit/and` a mask's bound, `nth` a computed index's interval; `:or` lowers to `(get m k default)`.
- **2026-09-17** — the checker's site walk is memoised per form across the derivation's rounds (`sigs::SiteCache`: 73% of walks reused, `nest check` over `std/` 11.0 → 8.8 s, verdicts identical by the new `derivation_cache_differential` gate) — and that gate found KI-158: the joint fixpoint was never a function of the file (a specialization memo outliving its round, returns re-read in hash order), the same file inferring `(int 0 2)` five runs out of six.
- **2026-09-17** — ADR-361: return-type dispatch declined, not deferred (a receiver-less op chosen by the context's type can only be a checker-driven rewrite; generic code names its target as a value; a designator-keyed multimethod is the door left open); tier-2 monomorphization queued as `perf-handoff.md` Task 5. The type-system list has no open item.
- **2026-09-17** — B8: every hash-collection iteration under `types/` audited (≈60); five reached a verdict or its order — the dead-clause lint's choice of binding (now the guarded symbol, then binding order), three ability/protocol finding loops and a `deftype` shape warning (sorted), the checker's discovered module loads (name order). New gate `crates/nest/tests/check_order_differential.rs`: `tests/` checked in two processes and in reverse order must agree per file, in sequence — and it found what the read could not: the `*methods*` registry is process state, so `<`'s domain (`number` + every `compare-to` record) grew with whatever earlier files had loaded (`count-up` derived `number` forward, `ordered` in reverse). A registration now counts only when its namespace (`*method-from*`) is in the file's require closure, closed through std.
- **2026-09-17** — B7: `nest check` and `brood --check` REFUSE (exit 2, reason and fix named) from a binary whose baked-in `std/` is older than the checkout it runs in — a checker verdict from the wrong std is wrong in both directions and reads like a right one; `nest test` keeps its warning, the two policies pinned apart (`crates/nest/tests/stale_binary_refuses_to_check.rs`). The pre-push hook and `make green` show the refusal line.
- **2026-09-17** — B6: a cap the checker hits is reported — one `checker gave up: …` note per file (`note:`, advisory, never gating) for a fixpoint that did not settle, a derivation declined over a same-file macro, the specialization fuel, the depth cap. Writing it exposed that Pass 2.8 never widened: 34 std/tests returns "still moving" after 16 rounds, each reading `any` at every caller silently; with the joint loop's widening/fold there, none. `check/tests/caps.rs`, sabotage-verified.
- **2026-09-17** — sound first (type-system list A1/A2/A4/A5): the runtime contract checks intervals, lengths and recursive types; a declared overload is checked against its body per arm (which found `(math/pow 2.0 0)` answering the int `1` — fixed in the code); strict reports a declared return the checker cannot verify as *trusted, not verified*, 43 std sites read one by one (32 fixed at the leaf, 11 acknowledged `(check-allow :trusted …)`); four never-seeded fixpoint holes and the `(int and (not 0))` exclusion misread closed on the way.
- **2026-09-17** — perf-handoff Task 5 (tier-2 monomorphization) ANSWERED and closed: ability dispatch costs ~370 ns per call and Tier 1 recovers ~260 of it, but no benchmark row and no real program has a site (0 devirtualizations on every row, 4 in bedit's whole suite) — no channel. KI-161 found taking the number: the KI-90 rebind guard never guarded (dead in modules, permissive without one); structural now.
- **2026-09-17** — a `cursor-zone` inside a region is hit-tested again (ADR-080 amendment, with ADR-363): the frame's zone collector read top-level ops only, so `cell-region`/`scroll-region` swallowed every zone inside them — a zoomed pane's links lost their hand cursor the moment `cell-region` shipped. The walk recurses with the paint's own coordinate math, stores zones as grid-relative PIXEL rects (a region's cell is not the window's), and tests the pointer per pixel, not per cell.
- **2026-09-17** — ADR-362: the `supervisor` row **0.88 → 0.66 s (−25%)** with no edit to the supervisor — the `receive` matcher chains clauses as an `or` (no fail thunks: five closures per message and a call per failing clause, gone), the type predicates are a prim (`vector?` 168 → 67 ns on the VM, 1 ns native), `%vector-ref`/`%vector-length` inline (the `VectorRef` entry had named the native by its pre-`seq/` spelling since the rename), and `MapGet` is default-on with a plain map's miss answered inline (hit 322 → 66 ns, miss 519 → 123).
- **2026-09-17** — C9 closed (ADR-364): the last two joint-coverage holes ADR-262 documented, both false positives at a call. A **record is a product** over its declared keys (`record_covered_by` on ADR-289's subset rule, so `{a: int|string}` ⊆ `{a: int} | {a: string}`) with the undeclared remainder deliberately not a position — it stands for unboundedly many independent keys, so it must fit one candidate's rest, the vector argument — and a **bounded interval of ≤64 values is the literal set it denotes** (`enumerated_int_range`, int-literal rule only), so `(int 1 2)` ⊆ `1 | 2`. The neighbours that must stay false (`map<K, A|B>` vs split maps, the 2-field componentwise case) are pinned beside each; both mechanisms sabotage-verified to red their own pins alone. 585/585 types, strict 0 + the 8 advisory notes, both checker differentials agree.
- **2026-09-17** — CI's last two reds closed (the checker's whole-tree differentials timed out in BOTH jobs — tier pin + `image_matches_source`'s budget, `669c580c`; bedit's ratchet, `03e955c6`) and the benchmark column refreshed on request at `a406f9a6` (brood-benchmarks `d2646f4`): FLAT, except `pipeline` +7.3% which is the pre-flight type check growing 131M → 144M instructions per file (the run +0.8%) — KI-150 reopened by the trigger its mitigation named. Found beside it, unexplained: `BROOD_NO_CHECK=1` runs `pipeline` at 2.8× the instructions.
- **2026-09-18** — C15's display half: the checker's purity walk graded a `:pure` DECLARATION and showed nothing anywhere; it is a question now (`properties::effect_of` → `%function-effect`) and `nest docs` prints an *Effects:* line — `(io/puts …)`, or `calls f, which performs an effect: …` through a call. The constraint that shapes it: `effectful_head` is a DENY-LIST, so the walk is sound for REPORTING an effect and not for certifying purity — the line appears only when there is an effect, and no page says "pure", because the absence is not a claim. An effectful PRIMITIVE is named directly (`proc/send` has no Brood body, which the first cut answered nil for). Totality across calls stays open: a call graph with a measure per edge is a different problem from the per-function `:total`. Also C17 swept and left waiting — no designator-dispatch need exists in std/ or bedit, which is the trigger ADR-361 named; and perf Task 2 half-answered from the bail trace (zero `deopt-thrash-latched` arms across all five in-tree float programs, so no NEW one can exist here whatever the pre-KI-114 binary had). Found on the way: the doc catalogue's `*fuzzy-scorer*` entry had been red — no `fuzzy/…` key existed, so the test's prefix loop never loaded the module that defines it.
- **2026-09-18** — C16, probed hole by hole: of the three the sweep noted, ONE was real, one was already closed, and one was wrong as written. Real: a **closed record's `count`** is its field count now (`[required, declared]`; an open shape declines) where it read `int[0..]`. The instructive part is where it must NOT live — putting the count in the type's `len` slot, the obvious one-line change, broke the gradual relation, because a length there is a refinement every RELATION reads and a bare `map` (the unknown shape, which must fit any shape) stopped being a consistent subtype of a closed record: `[0,∞)` does not fit `[2,2]`. Two existing tests caught it; it is read at the `count` CALL instead, and the pin asserts the relation is unchanged. Already closed: a computed `nth` index bounded by a guard — C12's offset bound did it. Wrong as written: "a string source gives `into` no length" — `into` does not take a string at all (`(into [] "abc")` RAISES), so the checker's `expects seqable, got string` is correct rather than a hole.
- **2026-09-18** — KI-165 fixed the same day it was filed: a `deftype` alias is enforced at RUNTIME now. `type-matches?` resolves the name through the new `%type-alias` (keyed — `%type-aliases` walks every declared sig, which a per-value contract cannot afford) and matches the type it names. Termination has two halves and the split is the whole subtlety: a recursion that CONSUMES VALUE always ends (a Brood value is finite and acyclic), so element/field/position calls start with a fresh `seen`, while the same-value ones (`or`/`and`/`not`/`len`/`rec`, the expansion) thread it — the first cut threaded everywhere and swallowed a bad leaf at depth 2, mistaking `tree`'s own `:l` descent for a cycle. Resolution ends with the checker's unique-suffix rule, because a contract runs where the VALUE is: a `test` body's green process does not resolve the bare name to the module that declared it. Found on the way and fixed with it: **`(rec X (or nil X))` crashed the checker** — a non-contractive μ (the reference unguarded at the body's top level) diverges on unroll, and one call site was a stack overflow; writable from any sig since ADR-349, and the alias spelling would have added three more shapes of it. Both go through `mu_if_contractive` now and answer `any`. Sabotage: the lookup disabled reds two tests; the cycle guard removed HANGS the file.
- **2026-09-18** — C14 (ADR-369): a self-referential `deftype` is a μ type now, so the two spellings of a recursive shape agree. `(rec X …)` was always exact at any depth; a bare self-referential alias was unrolled once and read `any` below that, so the author's choice of spelling decided how deep the checker saw — `(:v (:l t))` typed, `(:v (:l (:l t)))` not. `alias_ty` pushes the alias's own name onto the same `REC_BOUND` stack and wraps in `Ty::mu`. **One binder only** (`Ty` has a `mu` flag and a boolean `rec_ref`, no de Bruijn index — a reference inside two binders cannot say which it means), so the binding happens at the outermost expansion and only over a body with no `(rec …)` of its own: the exclusion the item reserved, and a representation limit rather than an oversight. Free with it: mutual recursion collapses to one binder by substitution when each name occurs once, exact at any depth. Three pins, sabotage-verified; the test that pinned the old bound is replaced. Found on the way and FILED as KI-165: no `deftype` alias is enforced at runtime at all — `type-matches?` has no alias case, so `sig!` over any alias accepts any value, every alias since ADR-327.
- **2026-09-17** — C13 (a float interval) DECLINED, not deferred (ADR-367), and the reason is soundness rather than effort: **floats here are not totally ordered and NaN is reachable** — `(* 1.0e200 1.0e200)` is `inf` with no raise, `(- inf inf)`/`(* inf 0.0)`/`(/ inf inf)` are `nan` (while `sqrt -1`, `0.0/0.0` and `asin 2.0` all raise), and `(< nan 1.0)` and `(>= nan 1.0)` are BOTH false. The int interval's ~117 call sites assume `¬(L < R) ⟹ L ≥ R`, which is exactly what a float breaks, so the "cheap on the int machinery" premise was wrong. The corpora ask for four prose-documented ranges and the one that matters (`stats/percentile`) already validates at runtime. What keeps today sound is that floats do NOT participate — `int_guard_ty` narrows to "an int in range, or not an int" — now pinned, sabotage-verified, so a future attempt meets the ADR instead of the shortcut.
- **2026-09-17** — C12, and its own advice followed: the corpora were surveyed before anything was built, and they show ONE shape — a computed index `(+ i k)`/`(inc i)` guarded by exactly that expression (`json` ×4, `ansi` ×2, `url`); `i < j` between two locals appears nowhere as an index guard, so no relational domain was built. `index_bounds` is now `i → {xs → k}` (`i + k < count xs`), a proved offset covering every smaller one — which reads json's `\uXXXX` scan, guarded at `+10` and reading `+4`/`+5`. Before it, a guard over `(+ i 1)` produced **no facts at all**: the comparison was discarded whole, so it narrowed nothing either. Found underneath: a `let` is bound in THREE places — the walk, inference, and the return check's `gradual_of_compound` — and only the walk recorded the count alias, so `(let (n (count words)) (if (>= n 4) (nth words 3) ""))` was clean as an argument and warned as a RETURN; one shared helper now, and the pre-existing alias test passes with the fix removed, which is why it survived.
- **2026-09-17** — C11 (ADR-365): `(seqable T)` — a sequence of `T` whichever shape carries it (`nil | list<T> | vector<T> | set<T>`), which is exactly the union `stats/median` and `stats/percentile` were spelling out by hand. The four members merge into ONE term, so no element machinery is new — a body checks the moment the spelling exists, and `stats/min`/`max` lose their `(check-allow :trusted …)`. `map` and `bytes` are deliberately out (their elements come from the kind, and six terms would overflow `MAX_TY_TERMS` and silently widen the element type away), so `(seqable T) ⊆ seqable`. Writing the runtime contract for it found `(set T)` had **no `type-matches?` arm at all** — a declared `(set int)` contract had been checking nothing since element types shipped — and that `set` was missing from `TYPE_HEADS`, so `(set)` read as an unknown constructor rather than an arity mistake. Five pins, sabotage-verified five ways.
- **2026-09-17** — KI-163 / ADR-366: a body compiled before its module lazily loaded kept its pre-load shape (generic call where the eager compile inlines `PrimOp::Rem`) for the whole process, and the JIT leaf upgrade read stale forever — the `BROOD_NO_CHECK=1` 2.8× oddity of the afternoon. The load that a miss triggers now marks the arm; the cache lookups evict and recompile it, and a running `SelfCall` loop adopts it through the hot-reload guard. `pipeline` no-check 591M → 184M instructions (eager 183M); the loop 302M → 163M. Two new dev probes, `%vm-arm-ops` / `%vm-arm-stale?`; two guards, sabotage-verified.
- **2026-09-17** — KI-150 measured, not guessed: the checker's Rust is ~3% of a `brood --check`; the cost is eager MATERIALISATION — `(seq/lmap …)`, a prelude binding, loaded `seq` + `map`/`math`/`reflect`/`string` (138M vs 75M). The eager drain now skips a module whose recorded names are all bound (138 → 82M; verdicts unchanged, both directions guarded). What remains (`math/rem`'s module pulling four more in, ~70M of `pipeline`'s 145M check) is the structural option: inferred std signatures carried in the image. Also: KI-162 was already closed by `3fd89869`; B7's std-tree hash costs ~10% of a check run INSIDE the checkout.
- **2026-09-17** — ADR-368: `(get m k default)` and `(assoc m k v)` on a map are instructions (`PrimOp3::MapGet3`/`MapAssoc`, VM arm + one JIT callback each): a loop of two reads 1124 → 172 ms per 2M (281 → 43 ns a read), an `assoc` loop 921 → 590 ms (460 → 295 ns). Differential over every branch at every tier + a settle-to-native check; `BROOD_NO_MAPASSOC` is the allocating op's own lever. Found on the way: a map literal inside a hot arm bails the arm (`MakeMap` is not in the JIT subset).
- **2026-09-18** — benchmark column refreshed at `04958398` (brood-benchmarks): `supervisor` 614 → 584 ms (−4.9%), the ADR-368 primitives' reading; 2.3× → 2.2× Elixir; every other row inside its spread.
- **2026-09-18** — KI-164 fixed: a declared `(is T)` guard never narrowed because `predicate_guard_ty` treated the file's own `defn` as a shadowing local (`is_local` includes file globals); `is_local && !is_file_global` now. Four guards with controls; the redundant-sig sweep exempts guard sigs, which it rendered as `bool` and would have stripped.
- **2026-09-18** — `MakeMap` joins the JIT subset: a `{…}` literal in a hot arm no longer bails the whole arm to the interpreter (it did, for good — found writing ADR-368's native check). Lowered as the variadic `MakeVector` is, one `brood_rt_make_map_n` callback over `map_from_pairs`, so both engines build the identical map; capped at 32 values. Guard `tests/jit_makemap_test.blsp` (value + tier, sabotage-verified).
- **2026-09-18** — the multi-pair `(assoc coll k1 v1 k2 v2 …)` is unrolled at compile time into nested single-pair `assoc`s, each a `MapAssoc` instruction (handoff item 2): no rest list, no `%assoc-map-pairs` loop; wrapper semantics kept (map, vector, last-wins, source-order evaluation), guarded by `tests/assoc_unroll_test.blsp` incl. the compiled shape. A three-pair `assoc` loop, 1M iterations, release-fast: **1285 → 531 ms** (1.28 → 0.53 µs per iteration), 612 ms at the VM ceiling; `make ab --floor` against `fac63929`: **`supervisor` −5.8%** (floor 0.3%), every other row inside its floor.
- **2026-09-18** — benchmark column refreshed at `a6a0d934` (brood-benchmarks): `supervisor` 584 → 553 ms (−5.3%), the unroll; 2.1× Elixir. Short rows +3–4% = the checker's per-file cost growing 4–7M with today's checker work, plus `os/which`'s `path` reference now materialising `path`+`file` under every checked `os/`-naming program (~18M) — KI-150, two more data points.
- **2026-09-18** — KI-150's coverage half: `&optional` was missing from `std_index::every_symbol_is_a_type_word`, so 14 declarations spelled entirely in the type grammar declined to ride in the image footer for a reason the grammar does not have (`string/pad-left`, `string/fields`, `reflect/type-aliases`, …) — both markers of `annot::arrow_of`, neither a name to resolve. Its gate asserts on the SOURCE SCAN first, because a footer-only assertion passes vacuously against an image built before the fix (which is how it first went green under sabotage). Then the gap was made readable rather than probed one name per run: `untyped_names_that_force_a_module_load` (an `#[ignore]`d triage test) reads the embedded sources and ranks, per module, the names other std modules reference and the index has no type for. Five `path` and eleven `string` signatures declared off that list — `string` is referenced by 32 std modules — and both modules drop out of the transitive scan: the `os/env` one-liner's pre-flight went 3 materialisations → **0**, the index 124 → 150 authoritative types. **And the saving is below the floor**: 107.23–107.27M instructions against a base of 107.25–107.36M, interleaved. Materialising from the image is cheap on this box, so what coverage buys is the smaller heap and a verdict that no longer depends on what else happened to load — not a faster check. (A first reading of +63% on the SOURCE path did not reproduce — the same invocation read 226M/336M/367M. The rig, not the code.)
- **2026-09-19** — KI-150's named next item, done and scoped: `cli_support::stdlib_tree_hash` — the staleness guard that re-hashes every `std/**/*.blsp` to tell you your binary predates your edits — cost **15.0M instructions, 16% of a release `brood --check`** on a one-line file. The 14% on record was a *debug* profile; callgrind on RELEASE agrees, and the A/B is clean because the guard is skipped outside a checkout, so one binary measures both arms (inside 93.88M → 80.61M, outside 78.92M → 78.43M as the control (the 0.5M baseline shift is build-to-build codegen, which is why the guard is read as the WITHIN-binary difference)). The cause was the loop and not the I/O: FNV-1a a byte at a time over 3.0 MB is 4.4 instructions per byte on a serial dependency that cannot vectorise. Same function over 8-byte WORDS with a byte tail and the chunk length mixed in first (`mix_chunk`, mirrored in `build.rs` and `cli_support.rs`): **2.2M, 6.8×**, and the symbol left the profile's top entirely (`Heap::env_get` is the new head at 12.3%). A `stat`-keyed cache would take the rest and was REFUSED — this guard exists because a stale verdict "reads exactly like a right one" (B7), and an mtime cache reintroduces that in the dangerous direction; the word loop is the same function computed faster, with no such risk. Both guards sabotage-verified (a line appended to `std/uuid.blsp` reds `brood --check` *and* `nest check` at exit 2; one `^ 1` of drift reds the agreement gate). **Scope stated in the entry: `brood file.blsp` does not call this, so the `base64` row KI-150 opened on is untouched** — the win is every checker and test invocation. Note for bisects: `BROOD_STDLIB_HASH` moves, so every cached stdlib image is re-keyed once.
- **2026-09-19** — KI-166 filed and its gate fixed: `cli::mapprim3_differential` failed one full-suite run with *"a prim changed an ANSWER"* and no answer had changed — the two arms printed the same map in different ORDERS. Reproduced deterministically: a map is a CHAMP trie, iteration order follows the keys' hashes, a keyword's hash follows its interned id, and an image boot interns in a different order from a source boot, so `(assoc {:a 1 :b nil} :c 3)` prints `{:a 1, :c 3, :b nil}` warm and `{:c 3, :b nil, :a 1}` under `BROOD_NO_PRELUDE_IMAGE=1` — same binary, same program. Delete `~/.cache/brood/prelude-expanded-*.img` and run twice to see it with no flag at all. The differential compared RENDERINGS across two processes; it compares by `=` now (content-exact, order-free, and it separates a stored nil from an absent key) with an `assoc-map true true true` pin so a wrong map in BOTH arms reds — strictly stronger than before, since the rendering was never pinned against an expected value. Sabotage-verified. Checked rather than assumed: `mapget_differential` renders only scalars, and the doctests compare parsed values, so the eight docstring examples pinning a multi-key map's rendering are safe (`doc_examples` 3/3 and `doctest` 13/13 under BOTH boot paths). Left open in the entry: whether the prelude image should restore the interner so ids match a source boot — unspecified order is a weaker promise than people assume, but it is the ADR-314 class that has bitten twice. **The transferable lesson: a differential must compare the ANSWER, not a rendering of it** — a rendering carries incidental state (order, float formatting, a pid, a timestamp), and when that drifts the failure accuses the feature under test by name.
- **2026-09-20** — KI-166 FIXED, and the fix was not where the entry expected: a symbol hashes its **spelling** now (`value::symbol_hash`), not its interned id. An id is an index into an append-only table — a record of *when* a name was first seen in this process — so hashing it made a map's iteration order a property of the process, differing between an image boot and a source boot. Hashing the spelling makes it a property of the program, which is what everyone already assumed. `NAME_HASHES` (FNV-1a per name) is pushed in lockstep with `NAMES`, and BOTH hash paths use it — the scalar fast path and the compound one, or a keyword nested in a vector would still carry intern order. **The part worth remembering: reading that table directly cost +15.8M instructions on boot (63.4M vs 47.6M with the lookup stubbed out), because `boxcar::Vec` is segmented — a bucket computation and an atomic load, right for the cold intern path it was chosen for and wrong for something every map and env op pays.** Reading through a per-thread `Vec` — the same shape, and the same reason, as the interner's existing name→id cache — brought boot to 46.90M against a 46.44M baseline (+1.0%, the interning FNV), with a map-heavy loop at ~1.20G against 1414.6M (−15%) since a keyword now skips `DefaultHasher` entirely. Callgrind, from outside a checkout so the staleness guard is skipped on both arms; **no wall-clock, because no benchmark work happens on this box** — queued in `perf-handoff.md`. Gated by the boot differential KI-166 itself named (`…render_a_map_the_same_way`: four rendered maps, keywords/symbols/strings, image arm against source arm), sabotage-verified by returning the hash to the id. Nothing else moved: no docstring example changed, because the doctest harness compares parsed values.
- **2026-09-20** — a "make sure everything is green" pass that found three reds, none of them in the runtime. (1) **`main` was red**: `stale_loop_handoff` failed CI's tree-walker job, because the property it pins is the VM's `SelfCall` → `ChunkExit::Tail` transition and at `BROOD_VM=0` there is no chunk, no guard and no transition — pinned to `BROOD_TIER=2`, the same fix and the same comment `mapprim3_differential` and `prelude_image_matches_source` already carry for that job. (The other half of that red was fixed in parallel by the machine owner: `math/max`'s call head loads at EXPANSION under a source boot, so the lazy path was never exercised; the program uses a load-path module now. Two independent causes, one symptom.) (2) **KI-83 recurred** — the mono differential failed over a **blank line**, where 2026-08-29's sighting was a duration line: the "Slow tests" block appears in whichever arm crosses a second, and that fix stripped the block's lines while leaving the empty line before it. `without_timings` drops blank lines now, which is the general form rather than a third instance-by-instance patch. (3) **The stranded-work watchdog's test asserted the machine's speed** — exactly one report, which is really "the 4.5 s program took 4.5 s"; the latch re-arms whenever anything runs, so one report per starvation EPISODE is the contract, and a run starved to 19.6 s reported twice, correctly. Bounded by `T/3 + 1` episodes now; with the latch removed the same run reports **4146** against a ceiling of 2. **The common shape, third time this week after KI-166: a gate that compares a RENDERING or a wall-clock accuses the feature under test when the incidental thing moves.** Verified on the merged tree: ten full-suite runs (1657/1657), `gcstress` clean, the tree-walker path 8/8, wasm 10/10, bedit smoke green, zero warnings from build, clippy `--all-features`, `nest check`/`--strict`/`format`, and the corpus/examples/imaged gates.
- **2026-09-20** — §7.9 closed: the float-slot veto in `plan_general_lowering` (a named self-tail arm with non-tail calls refused the general lowering when its profile showed a `Float` param) vetoed exactly ONE arm in the corpus — `mandelbrot`'s `row-sum` — and protected none (`nbody`'s refusals fail the self-loop test). Lifted behind a lever and A/B'd against a fixed baseline: every row inside its floor, and at `BENCH_N=1400` `mandelbrot` −4.3% instructions / −4.7% cycles (±0.24%) for +0.5M icache misses against 4.5G cycles. Removed, not narrowed (no measured case for a dynamic condition); `BROOD_FLOAT_VETO=1` restores it for a bisect; guard `jit_eq_join_test` §6 reads `:bailed` under the lever. Found on the way: `make tier-audit` is red on `supervisor` (`bench-supervisor/fill suspend-latched`) and was at `5fa10170` already — attributed in the handoff, not fixed.
- **2026-09-20** — the message path profiled (`perf record` + the timing counters, `pingpong` and `ring`): a message costs ~850 ns spread over ten items none above 25% — the interpreted receive-loop body (~180 ns; the arm hosts the receive so it is refused), the scan (~95), one matcher activation (~110), a per-receive `hof_resolve` (~45), the park/wake round trip (~150), the copy and the send (~85). Two levers, neither taken tonight: the receive as a native deopt point (§7.3's exit-point design — the loop bodies here are almost nothing but the receive, so the row gain is bounded well under the 21%) and a one-slot matcher-resolution memo (3–5%, declined over the ADR-366 staleness question). Written into `compute-frontier.md` §7.3 so the next session starts from the table.
- **2026-09-20** — **v0.31.0**, and the two gates KI-170's frame broke on landing: a directly loaded test file's defs leaked past its own isolate (the journal replayed them; direct entries are now scoped to the loading isolate — replayed for a bystander, discarded by their own) and a staged registry op did not bump `code_epoch`, so a `GlobalIc` served the first `defmulti`'s map to every later `defmethod` (deterministic on `multimethod_test` through the scoped runner; latent for any required module of that shape). Guard `nest::direct_load_frame`, sabotage-verified both ways. Release from a tree with `make test` ×2, the tree-walker differential, gcstress, seven fuzzers, breakage, every corpus gate, tier-audit, wasm and the pinned bedit smoke green.
- **2026-09-20** — the stability review of the day's work, and what it turned up. CI's `differential (tree-walker)` job was red on three commits: the day's two new Rust guards asserted a VM-compiler observable under `BROOD_VM=0` (the owner fixed `stale_loop_handoff`; `run_check_cache`'s replay probe pinned to `BROOD_TIER=2` the same way). The full battery on the final tree: `make gcstress` clean, GC_STRESS+VERIFY over the eleven JIT/lazy-load/handoff files clean, seven fuzz generators (metamorphic, arithmetic — whose unary set includes `-` — numeric, tier_transition, pipeline, linmap, match) 0 divergences / 0 crashes / 0 stale, `make breakagetests` clean, the tree-walker half of `test-both` 1664/1664 with CI's env, `make test` ×2. One flake, and it was a test race: **KI-172** — `spawn` then `monitor` on a child that dies at once reads `:noproc`; five tests moved to `spawn-monitor`. One review finding in ADR-372: the relower fired only on the sixteenth entry deopt, so an arm with TWO polymorphic params would box the first and deopt on the second forever — every sixteenth now, guarded by a two-slot case.
- **2026-09-20** — `nqueens` −28.5% wall (1 507M → 1 111M instructions) from one compiler rewrite: unary `(- x)` / `(/ x)` lower to `Prim2(Sub|Div)` over the identity — exactly the prelude's one-argument arms — instead of a generic `Call` to the variadic wrapper. `perf record` (usable here now) put 16% of the row in `brood_rt_car`/`_cdr`: `safe?` walked `placed` through callbacks because the `(- dist)` beside its reads was a NON-tail call, a GC safepoint that forbids hoisting the pair-slab bases. Every other row flat (`bintree`/`mandelbrot`/`nbody`/`fib` ±0.2% instructions). Guard `tests/unary_minus_test.blsp` (values on every number kind, the overflow promotion, the wrapper's errors, and the shape through `%vm-arm-ops`), sabotage-verified. Also settled: the `ring`/`pingpong` movement in the `1b9befd0` column is the release-fast codegen class (VM counters identical, icache misses +11%), and `target/ab` had filled the root filesystem — `make ab-clean`.
- **2026-09-20** — the column refresh, and two records closed by it. `brood-benchmarks` at `1b9befd0` (min of three interleaved brood-only invocations): `reduce` −21%, `spawn` −22%, `strings` −17%, `nbody` −14%, `pipeline` −14%, `supervisor` −10%, like-for-like 8.90 → 7.75. The first column at `4fb3e1cd` read base RSS 42.6 → 49.3 MB and that was ADR-371's: a hit skipped the walk's eager loads, the run loaded `io` lazily, and the top-level form recompiled stale (ADR-366) — a hit replays the walk's loads now (entry v2; the key hashes an explicit list of walk-changing flags, because hashing every `BROOD_*` made hits unobservable under a trace flag), 45.1 MB published. **KI-150 CLOSED** on the refresh's short rows. **Task 6 ANSWERED** by an isolated A/B of the symbol-hash change (`2fc5e13e` → `65598226`, fixed binaries, base-vs-base floors): neutral on every row; the map micro's win survives at the native tier (−17% instructions, −8% wall).
- **2026-09-20** — "fix any found issues", three: **KI-168** — `make tier-audit` was red on `supervisor` (`bench-supervisor/fill suspend-latched`, predating the day): `fill` lowered, hosted a `receive` three named hops down, dirty-parked once and was latched; `compile::arm_hosts_receive` now follows an arm's NON-tail call sites through named, already-compiled callees at tier-up and refuses by name (`hosts-receive`) before any compile — tail sites deliberately not followed (`via` still lowers), the latch stays the net for what the walk cannot see; 29 rows clean, guard `hosts_receive_fence.rs`. **KI-169** — found by fixing the `lazy_load_test` image guard (it matched `install: nil sections`): on a warm prelude image `BROOD_NO_STDIMAGE=1` reported `:installed 120` while reading source, because `%std-image-reinstall!` reset every snapshot registry except `*std-image-installed*`; one line, guard `stdimage_reporting.rs` case 4 (a prelude image written over a live stdlib image, then the opted-out warm boot). And `stale_loop_handoff.rs` had an image-dependent vacuity guard of its own — a std call head loads at expansion under a source boot (ADR-335), so its lazy load is a load-path module referenced as a value now, checked under image / no image / no prelude image. The lean build's three dead-code warnings gated `dev-tools`.
- **2026-09-20** — "try all three" perf leads, and each turned into a bug with a number: (1) **KI-167** — ADR-366's stale-loop handoff ran the recompiled body NESTED for the loop's whole life: `collatz` +6.5% / `sort` +5.7% instructions under lazy loading (a native preempt with no driver to yield to interprets up to 256 iterations), and a server loop whose first iteration lazily loaded a module parked 20 000/20 000 receives dirty; now a `ChunkExit::Tail`, guarded by `stale_loop_handoff.rs`. (2) The inline `empty?` deopted for anything but nil/pair, so every `(cond (empty? coll) … (rest coll))` prelude loop handed a vector/string/set deopted AT ENTRY per activation — `json`'s `needs-escape?` 7 910 per run, never latched (a `SelfCall` arm has no feedback); `brood_rt_is_empty` is the fallback, ADR-353's shape. (3) **ADR-371** — `brood file` replays its pre-flight verdict for unchanged text (content + build-id + stdlib-id + mode keyed; declines when a load-path module was loaded; `--check` never reads it): the walk was 18.7% of `pipeline`'s instructions. (4) **ADR-372** — found by the new `reason#` on `BROOD_DEOPT_TRACE`: `json/emit`'s param profiled Int on one activation deopted on every map, 4 819 per run; every deopt now feeds `jit_any_deopt_feedback`, and sixteen entry-tag deopts re-lower the arm with that slot boxed (three caches had to stop handing the old code back). `%jit-arm-state` gains `:deopts-total` because `:deopts` read 0 for exactly these arms. `make ab --floor`: `pipeline` −10.7%, `reduce` −8.3%, `supervisor` −7.6%, `strings`/`errors-deep` −6.5%, `json` −5.0%, nothing red. Note: `perf` IS usable on this box now (`perf_event_paranoid` = 1) and `valgrind` is not — `perf-handoff.md` says the reverse.
- **2026-09-17** — the pre-push hook has been INERT on this machine: a global `core.hooksPath` *replaces* `.git/hooks`, so `make hooks` installs a gate git never consults — and the override directory holds a deliberate `commit-msg` (which chains to a repo-local one for exactly this reason) and no `pre-push`. Found when an unformatted commit reached `main` through a gate that reported "installed". `make hooks` now WARNS with the path, `scripts/git-hooks/global-pre-push` is the chaining fix, and CLAUDE.md records the second half of it: `make prepush | tail` reports the PIPE's exit status, not the gate's.
- **2026-09-17** — C10 answered by probe and closed with no checker change: ADR-350's intervals and the int-closed/float-contagion rules had already taken the merely-wider residue, and the answer is a **mode split** neither mode shows alone — a *precise* mismatch (float contagion, exact division, an interval arithmetic cannot fit) is named in both modes; an *over-approximated* one (a call's result) is named under `--strict` and deferred in plain, the gradual valve. The residue itself lands there: `(sig f (number -> int))` over `(+ x 1)` IS reported under strict, because the declaration is part of the claim — a parameter admitting floats makes the promise false with no analysis of the body. Fourteen provable shapes silent in both modes, so the false positive it was left silent for does not occur. Three pins, sabotage-verified three ways (strict never/always applies reds the split in opposite directions; the return check disabled reds both warning pins).
- **2026-09-17** — KI-162: `nest check --fix-sigs` wrote every `sig` ABOVE its `defn`, the one placement `sig_placement.rs` forbids tree-wide. The locator reads the CST now (`sig-defn-sites`: root children, each node's newlines counted for the extent), so the sig lands one past the form's last line, a head laid out across lines is located instead of skipped, and "top level" is *root child* rather than *column 0* — a `check-allow`-wrapped `defn` still declines. Recorded beside the fix: the load failure the rule exists for **did not reproduce** (forward sigs over `defn`, `defn-` and a wrapped pair all loaded under contracts and enforced the contract), so the rule is what is verified, not the breakage.
- **2026-09-17** — and the two differentials split along the claim each already made (process vs order; `std/` vs `tests/`), since nextest's cap is per case and each was doing three or four whole-tree checks in one: 21–29 s per case under the tree-walker job's own environment, from 42–51 s, sabotage-verified (a per-call marker in `check()`'s output reds all four). The tier pin's 1.6× and the ~17% measured on the release binary are the same comparison against different binaries — a ratio is read with its profile.
- **2026-09-20** — "what is left to do?", answered by reading the queue and finding `main` RED: CI's `differential (tree-walker)` job at `d35cbdde` — where the defect ENTERED is `1b9befd0` two commits earlier, and the two runs between were cancelled by the next push, so a later commit wore the red (`make green`'s founding trap, live), one test — `cli::run_check_cache a_hit_replays_the_loads_the_walk_made`, ADR-371's own guard, failing on its **control** rather than its claim. The control waits for `[compile] stale-bindings` (a form compiled before `io` loaded, ADR-366) and the tree-walker compiles no chunk, so at `BROOD_VM=0` the line cannot appear for any of the three runs; the setup assertion fails and the test panics before testing anything. Pinned to `BROOD_TIER=2`, the sandbox's stdlib image built on the same engine, sabotage-verified **under `BROOD_VM=0` as well as the default** (replay stubbed → red both ways), which is the check that says the pin removed the vacuity without making the gate vacuous in the job that runs it. **Fourth sighting of the week's shape and the variant worth its own name: a gate must not assert an artifact of an ENGINE it is not about** — KI-166 was a rendering, the stranded-work watchdog was the clock, `stale_loop_handoff` was this one, same CI job. Then three things the handoff had recorded as impossible here turned out not to be. **`make tier-audit` ran, and it is green — 29 rows, every hot arm native.** What blocked it was never benchmark-row work: the corpus IS checked out, as `../brood-benchmark` (SINGULAR), while `ab-bench.sh`, `tier-audit.sh`, `jit-lower-witness.sh` and the Makefile all hard-coded `../brood-benchmarks`, the upstream repository's name. `scripts/bench-dir.sh` resolves either now. **The failure mode is the lesson: all three tools treat a missing checkout as a skip, not a failure — by design — so a directory name silently deleted a whole gate and the skip line was read as a fact about the machine.** A tool allowed to skip has to be sure what it is skipping. With the corpus reachable, **perf-handoff Task 2 is ANSWERED** (it never needed wall-clock, only the bail trace): zero `deopt-thrash-latched` arms on `nbody`/`mandelbrot`/`matmul`/`sort`/`primes`, only the by-design `call-mediated-boxed`/`call-spill-exhausted`/`chunk-outside-jit-subset` — and with zero on the current binary there can be no NEW one against any older one, so the KI-114 comparison closes by construction. Also settled, because it is a rig fact that costs an hour in whichever direction it is wrong: **`perf_event_paranoid` MOVES on this box** — the same day's `ring`/`pingpong` bisect was taken with `perf stat` at paranoid **1**, and measured later (ordinary user, outside any sandbox) it reads **4** and `perf stat -e instructions true` refuses, with no reboot in between (uptime from 2026-08-29), so something in userspace sets it and `sudo sysctl -w kernel.perf_event_paranoid=1` is the lever that does not stay. `valgrind` and `callgrind_annotate` ARE installed, whatever an earlier `command not found` reported — so callgrind is the instrument always available here and perf the one to CHECK for, never remember: `cat /proc/sys/kernel/perf_event_paranoid; perf stat -e instructions true; command -v valgrind`, and a number quoted from this box should name which took it. Three of the previous handoff section's four "not done / cannot be done here" claims had gone stale within the same day (the ratchet at `4d962e75`, KI-150 and Task 6 at `8710f4a0`) — the cost of a handoff written about work still in flight.
- **2026-09-20** — **KI-170**, the third mechanism behind the KI-119/KI-120 end state (`[refer] (:use set) imported NOTHING`, `unbound symbol: set` in a spawned child, `sexp`/`sse` alike, one loaded suite run): a module file reached by a DIRECT `reflect/load` — every test file the runner loads, every `load` from a tool — had `defmodule` `provide` its key at the top of the file and then ran in neither the ADR-344 staging frame nor the ADR-339 journal, so a concurrent `%isolate` snapshot between the provide and the definitions kept the provide and the restore lost the defs, and `require-one` short-circuits on `*features*`, so the one path that could repair it declines to. `require-one` was hardened three times against this window; the direct load kept "the immediate provide it always had" each time, documented as harmless because no *requirer* raced it — the racer was the isolate. `load` now wraps a `defmodule` file in the frame `require-one` uses: one publish, journalled; a plain script is untouched. Guard `crates/cli/tests/load_provide_window.rs` (a 700 ms gap between provide and def, an isolate across it, a `require-one` control in the same test), sabotage-verified: `features=true bound=false` with the frame removed. Every load/module/isolate file green, the `nest test` scoped runner green on four files, clippy clean.
- **2026-09-20** — KI-174 diagnosed in parallel with the fix that landed (`d7600bea`): twelve clean local runs; the first hypothesis (ADR-372's relower nulling a shared arm's code under peers' mirrors) was implemented and dropped — a stale mirror and a stale entry AGREE on the old code, so that shape can never answer `None`; bumping the epoch counter inside `jit_dispatch_fast_frame` reproduced CI's message byte for byte on the first run, which is the construction that named the TOCTOU. The landed fix (compare at the mirror's epoch, against the fat entry, with two deterministic unit tests) supersedes the one written here (hand `fl.epoch` down), which was dropped in the merge. Also answered in-session: the `reduce` benchmark row is 4 ms against Elixir's 31 ms because `%range-reduce` resolves the prelude `+` to a raw `i64` loop with no callback (5 ms at `BROOD_TIER=1` too — Rust, not the JIT); the row's C port documents that as the row's contract. What it hides: a reducer that must be CALLED costs ~33 ns/element (`defn` reducer 165 ms, 5.5× the BEAM), and `reduce` with a literal lambda is 130–139 ms where `fold` with the same lambda is 32 ms — ADR-360's fusion sees `fold` and not `reduce`, whose `& more` hides the literal. Both recorded for the owner's call.
- **2026-09-21** — "processes must be super cheap to spin up, hold and kill": measured first (200k processes, release): spawn+exit **1.66 µs** issue / 2.06 µs to dead, hold **4.4–4.6 KB RSS** per parked process, kill 0.7 µs unmonitored — and **1.2 ms MONITORED** (KI-176: every death walked the whole monitor table; a watcher-side index makes it 1.27 µs). Then a ~180 B/killed-process "leak" that was the default crash reporter's mailbox (KI-177: `:exit-abnormal` filtered only `:normal`, so every supervisor kill fed the reporter a message it discarded at 14 µs each and it fell behind without bound; the kernel now filters the reporter's own non-crash table). On the way: `main` was red twice — `d7600bea`'s strengthened mirror cross-check fired on a BAILED arm's mirror (`type_mixed_join_edges_stay_exact`, deterministic) and on a peer's earlier `(code, nslots)` snapshot of a swapped shared arm (twice per full suite) — both by-design states, both now tolerated by construction (a demoted entry; a debug ledger of every published pair), and twenty new public names were uncatalogued. Then the hold floor by histogram: every block of a parked process NAMED (`runtime-frontier.md` §B, refreshed) — and the first name was a two-month regression: `spawn` allocated a 328 B `ColdHeap` on every child to record an EMPTY package context (`spawn-live` peak RSS 1.73 → 1.62 GB, −6.6%, from a three-line guard), now ratcheted by `process_floor.rs`. Then the two structural items the histogram named: the four per-process maps became `SmallMap`s (one entry costs one entry, not hashbrown's four buckets — −388 B) and the root stack grows by four below sixteen slots (a parked receive's transient ninth root no longer doubles it — −96 B). **Parked process 4 690 → 3 868 B counted (−17.5%), `spawn-live` peak 1.73 → 1.52 GB**, every row within its floor (instructions flat on the two that read a few percent of wall). Still to do on this thread: spawn (1.66 µs vs ~1), the `Process` struct's 168 B of growth since July, and the reporter's own 14 µs/message. Then the day's last question — 100k files × 3k lines — measured on a 1 000 × 3k rig and written up in `docs/large-project-scaling.md`: warm `nest run` 3.0 s is O(source bytes) (the module index `read-all`s every file, twice — 2 000 opens on a warm run), whole-project `nest check` 161 s / 3.8 GB on one thread; running is fine at any size, the whole-project check is not; six-item queue, the module-index cache first.
- **2026-09-21** — the game engine's day, and what it asked of the runtime. `b2d-physics` (private) went from 176 to 24 ms a step at 500 bodies, and every gain but one was the language's: **ADR-378** — an arm computing on floats it reads out of vectors (`(dot a b)`) never went native, because the float context came from the param profile alone; deopt feedback re-tiers it after four integer-guard deopts on a `Float`, and on the way the thrash latch turned out never to clear callers' fast links (a latched arm entered natively forever), the compile cache handed re-lowers their old code, and comparisons sat outside the float context (they dispatch by tag now, a bool either way). `dot` 410 → 89 ns. Then the prelude: `conj` on a vector cost **7.7 µs** (`into` → `append` → two reverses → `apply vector`; `%vector-concat` copies once, 1.2 µs), `seq` 586 ns to return a vector itself (answers first now, as `fold` does), `reverse` reduces a vector natively; and the table: `get` rebuilds under the lock instead of cloning first, scalar keys compare as they are (six floats 204 → 144 ns). **ADR-374 addendum 4**: pong from an installed `nest` showed its score and nothing else — the CPU painter skipped `[:quad]`/`[:sprite]` silently; it draws them in software now, the same picture, and `b2d/run` says when there is no GPU target. "A trail behind the ball" was an `Immediate` present tearing; games default `:vsync true`. Downstream: `b2d/serve-shared` (one match for every window and tab, input stamped per player) and `pong2` (private) — physics court, three-level AI, online PvP, a disc sprite ball, 35 tests. Not measured on the rows: the three runtime commits (`perf-handoff.md` Task 7).
- **2026-09-21** — large-project scaling, item 1 (ADR-380): the module graph is a per-file cache on disk. A warm `nest run` re-parsed every source file WHOLE to find its `defmodule` header — twice (rooting, package identity), three times cold — which at 3 000-line files was the entire warm start (3.0 s at 1 000 files, 2 000 `openat`, the reader in `perf`). `std/tool/module-index.blsp` keeps `.brood/module-index` (one `("path" size mtime (mods))` line per file, atomic write, format-versioned, NOT build-id-keyed — it is a fact about source text) and `package`/`project` read the graph through it. Same rig, release: **warm run 3.0 → 0.15 s**, RSS 282 → 145 MB, source opens 2 000 → 0, cold 45.8 → 39.2 s. Gates: `nest::module_index` (the `[index] … 0 parsed` trace on a second run; an edit re-parses exactly its files) and `tests/module_index_test.blsp` (the answers), both sabotage-verified both ways. Found on the way: `scripts/bench/image-scale.sh` had driven ADR-325's OLD names for a month — fixed, plus `FNS=` for the 3k shape and warm columns. Next in the queue: incremental `nest check`.
- **2026-09-21** — ROADMAP 10 + 11, **ADR-381**: runtime contracts are BINDING-time policy. The kernel offers a binding to `%contract-wrap` at a `def`, at a `%register-sig` on a bound name, and in the sweep at `provide` (source and image alike; the prelude's root names at boot) — so `sig` is a declaration in every mode, placement is free (the tree-wide gate is gone), the stdlib image path is contracted, and a reload re-wraps; errors carry `:blame :caller | :callee`; `nest run`/`nest test` arm it (`0` opts out, a bundle never arms, `stdimage/build` refuses armed). Measuring it: every check ran TREE-WALKED (the machinery sat above its own macros in core.blsp — 600 µs a contracted call; now 3.5), `%type-alias` scanned every sig per non-base symbol (eleven base names were not interpreted; memoised), and **KI-178** — the JIT's inline `pair?` false on a range/seq-view since it was written. Prelude exempt under plain `1` (`nth` ×10 otherwise), `all` includes it. Residual `format_test` ×35 (`char-at` per char); module-boundary contracts recorded as the follow-up.

- **2026-09-21** — large-project scaling, item 2 (ADR-382, KI-179): `nest check` incremental for real. Measuring the doc's "unchanged re-check 16 s" from a cleared cache showed the ADR-119 cache hit only image → image — a user global's fingerprint fact is its DEF SITE, and a module from the project image had none (`F` vs `D<file>@<mtime>`), so the check after a cold build re-checked everything (154 s of 186 s) and so did the check after any edit (188 s). Three moves: the image carries def sites (`KIND_DEF_SITE`, the privacy-entry shape — also relights `source-location` / LSP go-to-def for imaged modules); an unchanged project REPLAYS its recorded lines without loading (keyed on the ADR-380 fingerprint + mode + `WALK_FLAGS`, now shared with ADR-371 through `%check-walk-flags`, + the listed files); the require graph and the lints' inputs no longer parse unchanged files (`%module-direct-requires` answers `:modules`, the index entry is v2). Also: `nest check FILE…` had never reused a verdict — relative keys against absolute ones. Rig, release: unchanged `nest check` **154 s / 15 s → 0.12 s, 118 MB** (a replay); one-function edit **188 s → 40 s**, of which 33 s is the image rebuild and 2.7 s the check (1 file re-checked); `nest check` over three listed files 9.3 s with 0 re-checked (4 s materialise + 4.6 s lints — the lints still scan every file); the require-graph parse 2.9 s → 0.04 s. Gate `nest::check_incremental`, sabotage-verified both ways. Left for the edit loop: the whole-image rebuild an edit still pays (30 s at 1 000 × 3k) — Finding 3.
- **2026-09-21** — "make 1000% sure we have stable ground": the merged tree's ARMED suite had four reds, and CI had been red for three commits. **KI-180**: `gui`'s handle registries were lazily `def`'d on first use — a check-then-define race that orphaned handles when two processes allocated at once (both `ui_test` reds; full suite only). Created at load; a 40-process guard. **KI-181**: an error raised in a contract shim's positionless code was reported at the CATCH site; `attach_vm_trace` now gives an untagged error the innermost positioned call site (non-tail: the caller's form; tail: the caller's caller). `vm_prim_error_pos_test` measured the thin-wrapper elision rather than the fused prim arm it guards — it calls `%table-has?` now, plus a `sig!`-forced shim case; `lazy_load_test`'s ADR-366 child runs unarmed. **CI**: `isolate_tests_run_alone` reads only the per-test `:isolated (test` spelling; the module-index test had it on the describe. Every guard sabotage-verified.
- **2026-09-23** — a checker false positive: hatch's `(when (web/audit/dev?) …)` warned "this test is always true" in every dev checkout. `dev?` returns `(def- *dev?* (web/env/dev?))`, read once at load; cross-file Gap A (`infer::global_value_ty`) typed the global by the literal it held in the CHECKER's process (`true` — `HATCH_ENV` unset), `dev?`'s inferred return became `true`, and the constant-condition lint proved the branch constant. The observation was a literal singleton, which is exactly the run-specific part. An observed value now contributes its KIND (`bool`, `int`, `string`), and an observed nil contributes nothing (the placeholder case — an unset env var reads nil too, whatever the name; before, only an earmuffed name escaped it). Messages read `got int` where they read `got 5`. Not done, on purpose: `is_earmuffed` reads the whole qualified spelling, so a module's `*global*` referenced cross-module (`mod/*x*`) is still typed — making it dynamic as well put ~10 `number`-vs-`int` strict findings into bedit (`(dec *width*)` and friends) for no soundness gain once nil is unknown. Gates in `check/tests/lints.rs` + `gradual_checks.rs` + `closure_inference.rs`, sabotage-verified. Residual, deliberately left: a truthy-kind global computed from the environment (a string set at check time, nil in production) can still prove a test constant.
---


## Recent — full entries

## 2026-10-14 — the soft memory limit stops the process holding the memory (KI-229)

Looking at whether `brood`/`nest run` should default a memory ceiling, the first question was
what a ceiling does to a program with more than one process. Under `BROOD_MEM_LIMIT=1G`, a
process keeping a 1 MB string every few milliseconds beside a busy worker that allocated only
garbage: the busy worker died with `E0043`, three runs in three, and the leak kept going. The
soft limit counts the whole allocator and raised in whichever process reached a safepoint
first, and a busy process reaches one far more often than a process that accumulates in short
bursts between receives. A default ceiling on top of that would make every leak kill its
bystanders first.

ADR-399: the crossing stops the largest holder. `Heap::footprint` measures what a heap holds
— both generations' slab capacity plus what entries own: spilled vectors, map and frame
arrays, string text, shared blobs, ropes (the existing `local_bytes` counts only slab slots,
and misses a 1 MB string entirely). Only a process may walk its own heap while it runs, so in
the last eighth of the limit each process measures itself at most every 50 ms, at its quantum
safepoint or when it parks; at the crossing the selector measures parked processes under
their mailbox lock, adds each mailbox's queued wire messages, and stops the largest with a
hard exit whose reason is the `E0043` error map, plus one stderr line naming it. On the leak
probe it measured 738 MiB for a process holding about 740 MB.

Two first attempts failed in instructive ways. Collecting fresh figures in a 20 ms window
after the crossing let a mailbox flood cover the remaining quarter to the hard limit inside
the window — an allocation abort. And without a measurement at park, the leaking process
(which runs for microseconds between `receive`s and never reaches the once-per-quantum
safepoint) published nothing, so the selector found no holder worth stopping and fell back to
raising in the busy worker — one run in three.

Guard: `crates/cli/tests/memory_pressure.rs` — a leak beside a busy worker and a flooded sink
beside a ticker, each asserting the culprit's `[:down …]` carries `E0043` and the bystander
is alive; ten runs green, and both red with the old raise-in-place restored.

The default-ceiling question is still open, but it is now a question about the number, not
about whether a ceiling would do harm.

## 2026-10-14 — native code memoises calls to a closure value (ADR-406); KI-239 filed

Handoff item A. A computed-head call from native code — `(f acc x)`, every callback a
higher-order function calls — re-derived the callee's arm, env and IC block on every call.
`jit_dispatch_call` now memoises them per closure instance and argc (eight direct-mapped
slots), stamped with the code epoch and the collection count; the table lives inside the
body cache's cell, whose mutable accessor drops it, so no write to the cache can leave a
stale memo. ADR-406.

- **Numbers** (`instructions:u` per call, steady state from a 1M/5M-iteration difference,
  both binaries with their stdlib image): anonymous closure 943 → 706, capturing 1 058 →
  817, a global function passed as a value 1 057 → 706; a named head unchanged at 415. Rows
  against a HEAD control: `persistent-map` −3.3%, `base64` −2.4%, `primes` −1.7%, `ring`
  −1.8%, `nqueens` −1.2%, the rest within ±1%; `spawn` read +4.8% once and is noise (six
  runs each: 1 225–1 390M on both binaries).
- **What a profile of the first numbers hid.** The obvious micro — `(apply-loop (fn (a x) (+ a
  x)) …)` — cost ~1 900 instructions a call, named or not, and the profile was all `dispatch`,
  `env_get` and `prim_add`: that body is a thin wrapper (ADR-069), which the dispatcher never
  runs — it redirects to `+` by name on every call. The call cache never sees it. A
  non-wrapper body is what measures item A. The wrapper cost is real and common
  (`(fn (acc x) (+ acc x))` is the canonical reducer) and goes to the handoff.
- **Two measurement traps hit again.** A first per-call figure of 296 was taken against a stale
  stdlib image (std loaded from source); and the first run after a rebuild pays the boot-cache
  rebuild, which skews a two-size difference. A HEAD worktree under `target/ab/` with its own
  image was the control that settled both.
- **The memory ratchet caught the first cut.** The table inline in every heap was ~600 B per
  process (`process_floor`: 4 510 against a 4 000 bound). Boxed and allocated at first use,
  then folded into the body-cache cell (no separate generation counter, no `RefCell` flag): 16 B
  per process. HEAD measured 3 976–3 985, which left 4 B of a 4 000 bound against ~9 B of
  jitter, so the bound is 4 020 with the sentence the file asks for.
- **Tests:** `tests/jit_computed_call_test.blsp` — instances of one template alternating
  through one call site, two templates through one site, one closure at two arities, a closure
  made fresh every iteration (collected and moved), and eight processes doing the same at once;
  each loop arm is settled native first. Sabotage: dropping the instance key reds four of five
  (the arity case is guarded by the argc key: dropping THAT crashes the process — the wrong arm
  with the wrong frame). Dropping the epoch and body-cache stamps together reds nothing and
  changes no instruction count, even with a module loaded mid-loop (ADR-366): native entry
  re-checks the arm's own `compile_epoch`, so a stale memo would be slow, not wrong — the
  stamps are recorded as an argument from construction, not as tested. At `BROOD_TIER=0|1|2`
  and under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`: 5/5. `make test-both`, `make gcstress`, fuzz
  (`tier_transition`, `metamorphic`, `pipeline`: 1 350 programs, clean), `make tier-audit` (29
  rows native), `frame_size_callsites` and the JIT/preemption/differential Rust tests.
- **KI-239 (watching).** `test-both`'s VM half had `a_dropped_send_to_an_unregistered_name_warns_once`
  fail try 1: A's `node/connect` was refused right after `wait_until_listening` had reached B.
  KI-99's test, a new mechanism. 40/40 on demand (20 with all cores busy); the test's failure
  message now says whether B was alive and still listening once A finished.

## 2026-10-14 (later) — merged origin/main; this branch's KI and ADR numbers moved

This branch (preemption, the native-entry refactor, the gate sweep, handoff items A and B) and
`origin/main` (v0.38.0 and its stability pass) were developed apart from `40d07ec0`, and both
minted KI-223 onward and ADR-399. Origin's keep their numbers; this branch's moved after them,
in one renumbering commit before the merge: **KI-223..232 → KI-230..239, ADR-399..405 →
ADR-400..406**. The commit messages written before it (`67a540e1` "ADR-399..404, KI-223..231",
`95c8b749`, `39710846`, `1869e297` "ADR-405; KI-232") carry the OLD numbers — read them through
this map. Conflicts were mechanical: both sides appended to the docs; origin made `is_dynamic`
per-runtime (`heap.is_dynamic`) where this branch had factored `push_bindings`; this branch had
moved the reduction ticks into `scheduler/quantum.rs` where origin edited their neighbours.

## 2026-10-14 (later) — deep data, refused threads, fast allocators, and a default memory ceiling

Four probes, four fixes (KI-240..243), and the default ceiling (ADR-407). Written on a branch
from `d7625f28` as KI-230..233 and ADR-400 and renumbered past the preemption merge before
landing; the code merged with `scheduler/quantum.rs`, where `expire_quantum` now lives.

**Deep data.** Every Rust walker a value reaches — printing, equality, hashing, sort, JSON,
send in both directions, `def`, table put and key, the collector, `postwalk`, `flatten` —
ran over a list, vector, map and pair chain nested a million deep, and sets 30 000 deep.
None crashed: each either finished or raised a catchable error (`json/encode` and
`seq/postwalk` hit the recursion limits). The finding was elsewhere: a dotted pair could not
be sent or stored at all, "improper list" from both copy paths, though the language documents
it as a value (KI-240). `Message::Dotted` carries the tail as the last item, so every walker
treats it as a flat container; the node wire gained a tag (protocol v10). Building a deeply
nested *set* is quadratic — each level hashes its whole element, 15 s at depth 20 000 — which
is a cost, not a crash, and is left as it is.

**Refused threads.** A user namespace (`unshare -U`) gives a program its own thread count, so
`ulimit -u N` there is an exact budget on any machine. Under it, `os/spawn` panicked after its
child had started, `offload` and the first `tcp/listen` panicked, startup panicked with a
backtrace, and with no scheduler worker the runtime hung without a word (KI-241). A child's
threads are now reserved before the child exists (`IoThread`); the offload pool and the
reactor start lazily and return the refusal as an error; the lazily-started singletons (JIT
compiler, heartbeat) no longer panic inside a `LazyLock`/`Once`, which would have poisoned
them for good.

**Fast allocators.** Building a test for a victim stuck in a native call turned up a larger
gap: the stuck victim's loop, allocating a megabyte an iteration, ran from the soft limit to
the hard abort by itself, because the VM checks the soft limit once per quantum and a native
loop once per 128 iterations (KI-242). The allocator now ends the thread's quantum on a large
allocation over the limit — `REDUCTIONS` is a `const` thread-local, safe to touch there — and
within the last eighth once per 1/64 of the limit, so a fast hoarder also measures itself for
the selector; a native arm hears of it when its builtin call returns. The first version only
fired over the limit, and the hoarder crossed the near band in fourteen iterations without
publishing a figure; the test hoarded to 350 MB under a 384 MiB limit and never entered the
band at all (352 MB is seven eighths of 384 MiB). The hard abort, when it still comes, now
says which limit and why, from a stack buffer — it runs inside the allocator. Then the stuck
victim itself (KI-243): the selector stopped nobody while its victim lived, and a victim in a
native call lives until the call returns; after 250 ms it is now skipped. The leak test of
KI-229 had to grow in pieces under 64 KiB: with the hook, a leak growing by megabytes noticed
the crossing itself and raised in place, which is right but no longer tested that a
bystander who notices does not pay.

**The default ceiling.** With no `BROOD_MEM_LIMIT`, a process in a memory-capped cgroup takes
its hard limit from the tightest `memory.max` on its path. Seven eighths of the cap was the
first choice and was wrong: under a 600 MB `systemd-run` scope the leak was out-of-memory
killed before the soft limit tripped, because the container counts resident memory and the
allocator counts live bytes (resident ran at about 1.45× live). At three quarters, caps of
600 MB, 1 GB and 2 GB each ended the leak with `E0043`; with the limits off it was killed,
exit 137, nothing printed. This machine's user slice carries a 20 GiB cap, so a plain `brood`
here now has a 15 GiB ceiling; the test runners keep their 3 GiB.

Also: `nest explain`'s entry for blocking natives advertised `(proc/offload …)`, which is
unbound; the function is the bare `offload`.

**Then, after the merge** — light probes only, the machine being in use. Descriptors: under
`ulimit -n 64`, `os/spawn`, `spawn-pty`, `os/cmd`, file reads and listens each fail with a
catchable `Too many open files`, and 200 children, 200 failed spawns and 200 `os/cmd`s in
sequence leak nothing. A full disk (`/dev/full`): file writes raise `No space left on
device`, the images are written to a temporary file and renamed, but stdout dropped its
errors and the run exited 0 (KI-244). Panics from Brood: the builtins' `expect`s are all
behind the type checks they rely on; the tree-sitter and GUI key-name tables took their
locks with `.lock().expect`, so one panic while holding one would have made every later
parse panic too — they use the poison-tolerant `core::sync::lock` like everything else.
Deep data in Brood: `json/decode` handles 100 000 levels; `json/encode` and `seq/postwalk`
raise a catchable `recursion too deep` at a million, and stay as they are — rewriting a
benchmarked encoder is not something to do without measuring it. The non-Unix `os/cmd`
starts its pipe threads with `Builder::spawn` and reaps the child on a refusal; checking it
through a `wasm32` build (also non-Unix) caught `1 << 62` in the new cgroup reader, which
does not fit a 32-bit `usize` — it reads the cap as `u64` now. A fuzz-differential pass on
the merged tree (`metamorphic`, `pipeline`, `tier_transition`, `trycatch`, `match`,
`strings`; 300 programs, four engine configs each): clean.

The pre-push gate on the merged tree then failed `doc_examples_test` on address space —
1 324 threads, all belonging to 317 zombie `sh` children. A logging `sh` on `PATH` named the
`os/spawn` example, run 484 times: a `receive` under `reflect/eval` suspended to the body
driver, which captured at the call into the native and re-ran the whole form on resume
(KI-245). `compile::run` was the one native entry to evaluation that left the capture flag
set; it takes the tree-walker's guard now. The plain double run predates the merge; the
merge's folds-in-the-driver extended it to a `map` callback, which is what the doctest hit.

## 2026-10-14 (later) — thin-wrapper callbacks from native code (ADR-408); KI-239 fixed; the format check ignores what git ignores

Four follow-ups to the merge.

- **Handoff item C (ADR-408).** A thin wrapper — `(fn (acc x) (+ acc x))`, the canonical
  reducer — never ran its own body: the dispatcher redirected it to `+` by name, and `+`'s
  two-argument arm is itself a wrapper of `%add`, so a call from native code took two by-name
  hops (~1 900 instructions) and the wrapper's arm never got hot enough to tier. Native code
  now runs the wrapper's compiled arm like any closure's (`jit_dispatch_call`'s slow path); it
  tiers, and later calls take the native link. Per call from a native loop: anonymous wrapper
  1 892 → 714, a global wrapper passed as a value 2 120 → 714. Rows, A/B of one tree with the
  path on and off: flat but `latency` (~−4% instructions, 5 runs each); `json` is bimodal
  (~1 588M or ~1 670M a run) on BOTH binaries — 11/5 off, 10/6 on over 16 runs, so not this.
  The contract boundary (ADR-383) holds because the compiled body applies it at compile time;
  `contract_test` gained the hot-loop case (a named wrapper, and a wrapper callback, inside
  the contracted module) — red under `BROOD_NO_CONTRACT_BOUNDARY=1`. That case first failed
  on a FIXED total: the neighbouring reload test swapped `f`'s body mid-loop, which is late
  binding working; it now asserts only that the boundary holds. `jit_computed_call_test`: the
  wrapper reaches `:native`, and a `def` of its inner function is seen through it — both red
  with the path disabled (the wrapper stays `:untried`).
- **KI-239 fixed — the test, not the runtime.** B lived for a fixed `(sleep 6000)`; under a
  loaded `test-both` A's boot outlasted it and dialled a node that had exited. Reproduced on
  demand by delaying A 7 s (old test: `Connection refused`, `left: 0`; fixed test: green). B
  now serves until killed and A waits for a pong — a link delivers in order, so the pong
  proves the drop warning was printed.
- **`nest format` skips git-ignored files.** The pre-push hook's whole-project check failed on
  `stress/fuzz_out/fuzz_6.blsp`, a fuzz output no commit can contain. `format-every-blsp` asks
  `git check-ignore --stdin` once and drops what it names (a tracked file never is; outside a
  repo nothing is dropped). In Brood, over `os/cmd`, not a new primitive. Guard: a throwaway
  git project with an unformatted file under an ignored directory passes the check and fails
  it without the ignore rule — red with the filter disabled.
- **`thread_exhaustion`'s no-worker case passed only without a stdlib image** (found by the
  post-merge `test-both`, red on both engines). It ran `brood` under a three-thread budget and
  expected no scheduler worker to start; whether one did depended on whether the JIT compiler
  thread had taken the third slot first, which it did only when the boot compiled something —
  loading std from SOURCE. With a live image a worker started and the runtime ran: tier 0 failed
  always (no JIT thread), tier 2 failed once the image was live, and a fresh worktree (no image)
  passed, which is how it shipped green. Two threads is the budget that means "no worker" on
  every tier and boot path (probed: all six combinations). Still a guard: with the runtime's
  `live == 0` exit disabled it hangs to the 60 s kill and fails.
- **`interp_teardown` gave the reap two seconds.** It failed once in a `test-both` tree-walker
  run at 2.655 s — its 2 s polling deadline plus setup — and passed 50/50 on demand (20 under
  full load). The reap of a dropped runtime's parked processes is asynchronous; the deadline is
  now 30 s, which a pass never waits for. `process_floor` also went red at `ae4e69a3`: the
  other line's KI-246 hash memo, fixed by its own `d6c3db0a` (8 bytes a process, not 72) — which
  left the figure at 4 020–4 021 against a 4 020 bound, two 8 B memo pointers on top of 3 996. The
  bound is 4 040, with the sentence the file asks for.
- **The renumbering commit stripped zero-padding** (`a279e76c`): its regex re-printed every
  matched number as an integer, so `ADR-073` became `ADR-73` in the 53 files it touched — 1 254
  lines, already pushed. The check that commit ran normalised numbers before comparing, so it
  could not see it. Repaired by re-deriving the correct renumbering of the pre-renumber text and
  applying only the differing lines; verified exactly for the 31 files the merge did not also
  touch, and line by line against both sides for the other 22 (one half-repaired line fixed by
  hand).

## 2026-10-14 (evening) — nested-set hashing, tables under the memory limit, a resized container

**Quadratic hashing (KI-246).** A set or map's structural hash walked everything below it
every time, so nesting `n` levels cost O(n²): 15 s at depth 20 000. Keeping a running hash
per CHAMP node was the other design — it would have touched every trie operation and, for
maps, hashed every value on every `assoc`. Instead `HashMemo` remembers the hash of a LOCAL
set or map that took 64+ steps, stamped with `gc_runs`; small keys pay a counter increment.
Measured by the hash's own step count, not a clock: 3 000 levels cost ~100 000 steps (each
level re-walks at most down to the nearest remembered one), against 4 504 500 without it.

**Tables.** A table belongs to no process (ADR-107), so the soft limit can stop nobody to
free what one holds, and its `E0043` named nothing. It now measures the tables
(`table::largest_table`, the stored entries' message sizes) and names the largest in the
hint. Filling a table with megabyte strings first raised through the "the current process
is the largest" branch, not the unattributable one: the filler's own heap still held its
copies of the shared blobs until the next collection. Both branches name the table now.
Owner-death collection of tables stays a language question.

**A resized container.** The cap was read once at startup; a pod resized in place kept its
old limits, and a lowered cap left them above it. It is re-read at most every 5 s. The first
hook, the VM's once-per-quantum check, never ran in the probe — its loop ran native and
otherwise waited in `receive` — so the native fresh-quantum point and the park re-read it too.
Cutting a `systemd-run` scope from 2 GB to 600 MB while the program ran moved its hard limit
from 1 536 to 450 MiB within seconds.

Also: ADR-406's closing sentence about thin wrappers, re-worded in the session that added
ADR-408, had landed as a loose paragraph after ADR-407; it is back in ADR-406.

CI then failed `process_floor`: a parked process cost 4 078 B against its 4 020 B floor —
the memo's `RefCell<HashMemo>` and work counter sat inline in every heap (~72 B), and every
spawn pays for the `Heap` struct. The memo is now a `OnceCell<Box<…>>`, allocated the first
time a hash is worth remembering (8 B inline), and the work counter is a thread-local. The
floor test had not been in the targeted set run before the push; it belongs there for any
change to `Heap`'s fields.

## 2026-10-14 (night) — a simplification sweep, and three order-dependent failures fixed

A whole-tree pass for code more complicated than it needs to be; behaviour unchanged except
for one gate bug. **Rust:** the unrequired-module lint (KI-17), a permanent no-op since the
ADR-227 follow-up, is gone with its `Ctx` fields — the require sets it collected still seed
the file's world (B8); `pop_dynamic`/`record_def_site` (no callers) deleted; copy-pasted
helpers merged (`Heap::list_or_vector_items`, `Message::local_pid`, the LSP's `chain_to`,
`expect_symbol` at a dozen hand-written sites, one name-query helper in `tooling.rs`);
`FinishBytes` and `BuildIdentityHasher` replaced by a fn argument and `BuildHasherDefault`;
`brood`'s three checker entry points are one; `nest`'s `run`/`ensure_stdimage` wrappers folded.
**Brood:** ~230 lines of hand-rolled loops replaced by the prelude/`seq`/`string` functions
they re-implemented, plus dead privates and leftover `(map … identity)` copies.
**Gate fix:** `check-stress.sh` passed a stress test whose summary read "10 failed" (a bare
`0 failed` grep, ignoring the exit status) — the bug `check-examples.sh` had already fixed.
Both scripts now share `gate_matches`/`gate_report`/`gate_suite_passed` from
`scripts/lib/gate-binary.sh`. `make bench` (an unreferenced alias of `make benchmark`) is gone.

**Three order-dependent failures, all on `8539f335` too, now fixed.**
- `registry_test` + `lazy_load_test` in one run failed `lazy_load_test.blsp:209` every time
  (KI-249): a module a test file loaded after its own `defmodule` kept its bindings but lost
  its `*features*` entry at the file boundary, because the nested load's `provide` was staged
  into the file's own direct frame and discarded with it.
- `contract_test` failed under 4 of 5 `--seed`s alone: its reload test redefined the shared
  boundary fixture's `f`, and the tests beside it read the reloaded body when it ran first.
  The reload test has its own fixture module now.
- `lazy_load_test.blsp:71` failed once in a seeded batch: two other tests loaded
  `lazyprobe-a`, whose load writes the marker file the operand-reference test asserts is
  absent. They load a marker-free `lazyprobe-plain` instead.

The first two surfaced only because a batch was run with `--seed`: in the default order every
one of these files passes. A seeded run of the files a change touches is cheap and worth doing.

**Second pass** (same night, per area). *eval/jit:* the JIT fallback callbacks' park-or-answer
tail is one `answer_or_park`; the out-slot reads in `jit_lower` are one pair of helpers; the
VM driver's 8 frame literals and 6 capture returns are `current_frame!`/`return_captured!`;
`CompiledArm::dbg_label`/`dbg_opcodes` replace ~23 copies; the three fold pre-check walkers
share `form_has_call`; stranded doc comments on `JitBackend` and `tail_of_cons` are back on
their items. *types:* one `walk::unquote` for five copies, `sig_arity` (a copy of
`arity_of_sig`) and three display forwarders gone, and the inert `:unrequired` `check-allow`
category removed with the lint it once suppressed. *core/builtins:* hand-written type checks
moved onto `expect!`/`expect_string` (`file/spit` and `os/run-process` now say "expected
string" like their siblings), `os/spawn`/`os/spawn-pty` share `spawn_spec`, the table and
arm-probe primitives share their argument preamble. *runtime/binaries:* a third LSP copy of
`chain_to`, `introspect.rs`'s eval-copy-reclaim and file-context boilerplate, the printer's
two range writers, the reactor's plain/TLS registration, `Message::nests`, and `nest`'s
duplicated runtime setup. *std:* `task-run`'s two `receive`s are one, `reconnect-boot` is
gone (the watcher starts in the down state, so a failed first connect no longer retries
once before the backoff), `format-spec` binds its character once, auto-gensyms in
`defmemo`/`bench`, stale argument-order comments in `seq` corrected.

**Third pass** (across areas, and the parts nobody had looked at). *Rust across crates:*
`Node::head_symbol`/`head_keyword`/`symbol_at` on the CST replace a dozen hand-rolled head
tests in `syntax/scope` and the LSP; `introspect::string_literal` replaces five ways of
quoting a string into Brood source; `eval::macros::form_items` and the checker's
`let_bindings` were copies of `Heap::list_or_vector_items`/`walk::bindings`; 14 hand-written
`OnceLock` env readers use `debug_flags::is_set`/`is_armed`; `kw::LAMBDA` was dead.
*Rust tests:* each test crate has a `tests/support` module — the `TempDir` struct alone had
27 copies. *Scripts:* `green.sh` judges binary freshness through `scripts/lib/gate-binary.sh`
(its inline copy missed `-dirty` shas); `scripts/suggest-renames.blsp` is gone (superseded by
`nest check --fix-renames`); the Makefile's nextest check, sanitizer prefix and binary list are
each written once. *Brood:* `markdown`'s block closers share one emitter (output compared
byte-for-byte over every `docs/**/*.md`), `project-check`'s seven root lookups and `docs`'
three entry setups are one each, `test`'s insertion sort is `sort-by`, test files use
`assert-error` instead of five private `threw?` variants.

Two bugs fell out. The `:slow` test report printed each row as `(123) ms` (`(rest entry)` on
a pair). And `reflect/references-in-source` — the MCP `callers` tool — had its own
`line_starts` that broke only on `\n`, so after a lone CR it reported lines early; it now uses
the reader's `scanner::line_starts` (guard in `tests/introspection_test.blsp`, red with the
old definition).

**Follow-up: three duplicates that needed a public name.** `editor/lexer/lexer-walk-lines
src line-spans` is the line walk `lexer`, `shell`, `dotenv` and `dockerfile` each carried
(dotenv/dockerfile's `lead-ws`/`find` guards were redundant over `string/span`/`index-of` and
are gone too; those two modes now load `editor/lexer`, and with it the face registry).
`project/require-root who &optional from` is the find-the-root-or-raise opening of `check`,
`run`, `format`, `mcp`, `package`, `docs` and `update-tooling` — one message now ("not in a
Brood project"; `package` said "not inside", `update-tooling` said something else again).
`docsite/render-refs text render` is the `[[name]]` scanner `docs` and `docsite` shared but
for the rendering. The fourth candidate, a shared HTML escape for `markdown` and `docsite`,
is two copies of three lines and stays as it is.

**Follow-up: the suite's copied helpers.** `tests/support/suite-helpers.blsp` (on
`:source-paths` beside `corpus`) now holds what test files had each copied: the JIT `settle`
loop (6 files, two variants; the one that sleeps on `:queued` won), the span `ascending?`
(dotenv, dockerfile), `sibling-binary` (the beside-the-runner-or-one-up lookup, 10 sites in 4
files), and the backpressure tests' mailbox measuring (`queued-while-idle` and its parts,
parameterised by message tag and chunk size).

## 2026-10-14 (night) — a named wrapper links from native code; `reduce` takes the counted loop; `range` is multi-arity

`perf_event_paranoid` is 1 on this box tonight, so the session started from `perf stat` /
`perf record` on the row furthest behind its peers (`nqueens`, 89 ms against Elixir's 11) and
on microbenchmarks that isolate one call shape each, differenced between two N so startup
cancels.

**A thin wrapper called by name from native code never linked** (ADR-408, amended). One
native loop calling a helper, instructions per call: `(defn add (a x) (%add a x))` 29
(inlined), `(defn add (a x) (+ (+ a x) 0))` 424 (linked), `(defn add (a x) (+ a x))` **2 139**.
`BROOD_PERF_STATS`: `jit_link_done` 0, one `vm_apply` and six `env_get`s per call. The caller
runs interpreted until it tiers, and the interpreter caches a thin wrapper on the call IC with
no arm (it redirects to the inner head instead); `jit_dispatch_call` took an arm-less hit for a
builtin and returned nothing for a closure, so the IC kept hitting and the miss branch that
attaches the arm — the only place a link is published from — never ran. ADR-408 had measured
the wrapper passed as a value, which is the computed-head path. Now 420 a call. The corpus does
not call named wrappers from native loops, so no row moves; user code does constantly.

**`reduce` over a range with a literal** was not the counted loop. ADR-360 §7 rewrites `(fold
(range …) INIT (fn (acc x) …))` into a `letrec` loop the JIT runs native, and a fused pipeline
under `reduce`, but not a plain `(reduce (range n) 0 (fn …))` — which is what the docs teach
and `nqueens` writes. Found by swapping the word in the benchmark: 1 206M → 936M instructions.
`reduce`'s three-argument form is `(fold coll init f)`, so the rewrite takes either head.
`nqueens` **113 → 90 ms** (`make ab --floor`, 0.0% floor, −21.8% instructions); every other
row flat by instruction count. std writes `fold` 344 times and `reduce` 17, so boot pays
nothing measurable for the extra body copy.

**`range` is three fixed-arity clauses** instead of `(a & more)` + `cond`: no rest list, and
each clause is one `%range` call. −1% on `nqueens`. `(range 1 2 3 4)` used to drop the
fourth argument silently; it is an arity error now, and `nest check` says so. A multi-arity
function has no single inferred parameter signature, so the checker lost "the bounds are
ints" (`inference_precision::get_in_with_a_literal_path_reads_the_declared_shape` caught it);
`range` declares what the variadic definition inferred, `(sig range (int &optional any any
-> list))` — typing every bound `int` flagged `(range 1 (+ n 1))` over an untyped `n` in
`tests/resolver_test.blsp`, a tightening that does not belong in a perf change.

What is left on `nqueens`: ~49% native (`safe?`'s list walk is the real work), and `solve`
itself interpreted (~25%) — it builds the loop's closure, `MakeClosure` keeps it out of the JIT
subset, and the gate would refuse it anyway (`call-mediated-boxed`, compute-frontier §7.1).
One trap re-learned: after a `std/` edit the `release-fast` `nest` must be rebuilt before
`nest stdimage`, or it writes an image under the OLD id and every run source-boots (1.63G
instructions on `nqueens` instead of 0.94G) — `(stdimage/status)` says `:stale`.

**Two memory-limit findings from the tier-0 half of `make test-both`**, both reproduced at
`8539f335`, so neither is from tonight's changes. KI-247, fixed: a holder that raised in itself
and died left its memory on the count for a moment, and the main process, waking for its
`:down`, raised in its place — `stopped.retain(is_alive)` dropped a dead victim at once, and a
self-raise was never recorded, so the grace that exists for "its memory is about to go" did not
apply. KI-248, open: at tier 0 an imaged boot builds a 500 MB `str` past the hard limit that a
source boot aborts on; the suite usually source-boots that test, which is why it read green.
Attributing these took one discipline worth repeating: a test needing a dev-tools primitive
(`%gc-collect`) cannot be reproduced with a release binary at all — compare the baseline
worktree's own test build, not a release-fast A/B.

**KI-248, fixed the same night.** Not an imaged-boot or tier-0 bug at all: the hard-limit check
read a live-bytes snapshot refreshed every 512 allocations a thread, and a 100 MB string made
since the last refresh was invisible to it, so a 500 MB `str` on top passed a 512 MiB limit.
The boot path only decided whether enough allocations happened afterwards to refresh it. A
large (≥64 KiB) or near-limit allocation now checks the exact shard sum.

## 2026-10-14 (late night) — the CLIF verifier off in release; a fold's callback finishes inline

**Picking the next candidate by profile, not by the menu.** A symbolized `perf` of the rows
furthest behind their peers. Two things on `base64`/`sieve`/`primes` were not Brood: `printf`
into a `File` from the JIT thread — cranelift-jit's `/tmp/perf-PID.map`, written only when
`PERF_BUILDID_DIR` is set, which `perf record` exports to its child, so a profiling
artefact (and 2 000 such files litter `/tmp`) — and Cranelift's verifier at 3–4.5%.

**The verifier, re-attempted.** Compute-frontier §7.2 rejected turning it off on 0.133.1 (a
`remove_constant_phis` assert on `json`, the JIT then off for the process) and said retry on a
Cranelift bump; the tree is on 0.135.1. Off in release, on under `debug_assertions`. Clean on
every check: no `CODEGEN-PANICKED` in any row (output identical to HEAD), in the whole Brood
suite run on a release `nest`, or in 1 800 fuzz programs — run against the RELEASE binary,
since the armed one keeps the verifier on. Pinned `base64` −10.8%, `json` −11.4%; unpinned
−4.5% / −3.6%, `regex` −2.3%. `strings` reads +1.7% pinned (23.19 → 23.58 ms) — it compiles
NO arm, so nothing changed is on its path, and a control build (HEAD plus a comment) read
+0.5% on the same measurement: build-to-build layout, under the regression bar, recorded.

**A fold's native callback finishes inline.** A fold over a vector with a non-primitive
callback cost ~750 instructions an element, the callback's own code ~7% of it. Two cuts:
`hof_apply_native` finishes a value itself instead of calling `finish_nested` per element (its
outcome 0 is one truncate), and `jit_verify_active` is one cached flag rather than two lazy
reads through two calls on every Rust-entered native call. 746 → 617 instructions an element
for a literal callback, 792 → 671 for a named one; `pingpong` −2.3% (its receive matcher
takes this path), the rest of the corpus flat. A per-span "session" that installs the
activation state once was weighed and NOT built: the corpus spends little here, and it would
reshape ADR-403's per-element preemption contract for a few hundred instructions.

**A hypothesis refuted on the way:** an allocating callback did NOT fall off the HOF fast path
after a collection moves its closure (`jit_link_done` 992k of 1M elements). And
`tests/lazy_load_test.blsp`'s ADR-366 case fails under a release `nest` — it calls the
dev-tools `%vm-arm-ops`; CI's test build has it, so not a bug.

**A computed-head call from native code, 771 → 652 instructions** (ADR-406 amended). The
same inline outcome-0 finish as the fold callback, and the link branches of
`jit_dispatch_call` probe the stack only from depth 64 (they count the level through
`enter_native`), where the slow path and direct builtin calls still probe every call. Measured
with live images and N 2M vs 10M — at 1M vs 3M with stale images the per-call figure wandered
±30 instructions, larger than the step being measured. Deep recursion through a computed head
pinned: 200 000 levels complete and 2 000 000 raise, in a spawned process, both binaries alike.

## 2026-10-15 — dependencies to latest; MSRV 1.96

`cargo update` across the lockfile, plus the majors: cranelift 0.135 → 0.136, wasmtime 47 → 49,
brotli 8 → 9, x25519-dalek 2 → 3, rodio 0.22 → 0.23, pollster 0.4 → 1, libloading 0.8 → 0.9,
wasm-bindgen 0.2.128 → 0.2.129. No source change was needed. wasmtime 49 is built on the same
cranelift as the JIT, so the lockfile carries one cranelift instead of two (−240 lines).

**MSRV 1.95 → 1.96:** wasmtime 49 and cranelift 0.136 declare `rust-version = "1.96.0"`. Checked
with `cargo +1.96 check --workspace --all-targets`.

**One break, in a test fixture:** the newer `wast` rejects the legacy component export
reference `(memory $i "mem")`. The form is now `(memory (core memory $i "mem"))`, and `realloc`
likewise takes `(core func …)`. `tests/wasm_test.blsp` was the only place that spelled it.

**The wasm-bindgen pin is read, not restated:** CI's install step and both wasm scripts now
take the version from `crates/playground/Cargo.toml`. `wasm-suite.sh` also refuses an installed
CLI that does not match the pin. Two scripts had still said 0.2.100.

Not taken: num-bigint 0.5. `bigdecimal` 0.4 and `num-rational` 0.4 are built on num-bigint 0.4,
so moving it alone would split `BigInt` into two incompatible types.

Verified: clippy `--all-targets --all-features -D warnings` on 1.99; `wasm-suite.sh` 10/10;
`wasm`, `wasm_sandbox_limits`, `zlib`, `crypto`, `conformance_crypto`, `grammar`, `tree_sitter`
and the `jit_*` Brood files; `jit.rs` 40/40; `distribution.rs` 39/39.

## 2026-10-15 (later) — six bugs from the duplication review (KI-250..255)

A whole-project review for duplication turned up six bugs. Each sat where the same logic is
written more than once, and one copy had drifted:

- **KI-250:** `spawn_pty` dup'd its master after the spawn, against its own comment. Its plain
  sibling did not. The registration both spawns repeated is now one infallible `register_child`.
- **KI-251:** `image_load_section` restored `current_file` by hand at four exits and missed the
  two error exits. It now has one restore around the whole body.
- **KI-252:** `nest` read argv with `std::env::args()`; `brood` had already switched to `args_os`
  for exactly this. Both now use `cli_support::args_lossy()`.
- **KI-253:** a panic in the timer thread would have ended every timeout. Each wake is now caught.
- **KI-254:** two deliberate copies of `file/walk-files` lacked its symlink rule.
- **KI-255:** 52 hand-typed error prefixes named functions that do not exist. The label test
  now reads message prefixes too.

Three of the six guards are arguments from construction: a dup failing at exactly one instant,
a define failing mid-section and a wake panicking cannot be produced on demand. Each fix is
shaped so that the bad state has no path, and the comment at the site says so.

A tooling trap found on the way: the Edit tool trims trailing whitespace from a replacement,
so a prefix rewrite ending in `": "` produced `"name:message"` 51 times. The new test missed it
because it splits on `": "`. Grep for the joined form after any such rewrite.

## 2026-10-15 (later) — tooling: flakes fail locally, pins agree, a supply-chain gate

- **`make test`/`test-light`/`test-both` run through `scripts/nextest-no-flakes.sh`**, as CI
  does. A retry-only pass used to read green locally and red in CI.
- **cargo-nextest 0.9.148 everywhere.** One nightly job floated on `install-action@nextest`,
  and `make ensure-nextest` installed `latest`. The Makefile's `NEXTEST_VERSION` is now the
  pin, and `crates/lisp/tests/tool_pins.rs` fails when any pin disagrees or floats.
- **`.PHONY` lists every target** (it had 26 of 49). `tool_pins.rs` fails on a missing one.
- **Actions:** checkout v7, upload-artifact v7, cache v6, action-gh-release v3. Their breaking
  changes are the Node 24 runtime and `pull_request_target` defaults; no workflow uses those
  triggers.
- **New CI jobs:** `msrv` (`cargo check` on the `rust-version` Cargo.toml declares, read from
  the file) and `deny` (`cargo-deny`: RustSec advisories, licenses, sources; `deny.toml`).
- **`.github/dependabot.yml`:** weekly, one grouped PR for cargo and one for actions.
- **cargo-deny's first run:** `rustls-pemfile` is unmaintained. Its job is done by rustls's
  own `pki_types::pem`, so it is gone. The two TLS error paths it fed now have tests
  (`tls_test.blsp`, sabotage-verified). `ttf-parser` (via cosmic-text) is ignored with its
  advisory id. `brood-playground` declared no license; it now inherits the workspace's.
- `install.log`, a committed build transcript, is removed and ignored.

## 2026-10-15 (later) — one HTML escape; the suite's copied helpers, round two

- **`markdown/escape` is the one HTML escape**, and it now escapes `"` as well, so its result
  is safe inside an attribute. `markdown`'s private `md-escape` and `docsite/escape` were the
  same three replacements, and neither handled `"`. `docsite` feeds names into `id="…"` and
  `href="#…"`, so the gap was one bad name away from broken markup.
- **Five more helpers in `tests/support/suite-helpers`:**
  - `expand-error` (3 copies) and `hint-of` (2). The `reader_hints` copy returned the
    evaluated value on success, so its `(is (nil? (hint-of …)))` held whether the form failed
    without a hint or ran fine.
  - `checker-messages`, which had 6 copies under 5 names, some returning a vector and some
    a list.
  - `late-messages` (2).
  - `next-seed`, replacing 4 copies of an LCG whose low bit alternated: every
    `(math/rem s 2)` choice in the generative tests took the same branch every other draw.
    `next-seed` is `rand`'s xorshift32. The generative tests pass on the new streams.
  - The `stress/` copies stay, since that harness does not load `tests/support`.
- **Left alone, on purpose:** `test/opt?`/`opt-val` step one element at a time. The run-tests
  option list mixes bare flags (`:trace`) with key/value pairs, so a plist reader would
  misread it.

## 2026-10-15 (later) — `project/source-files`; `codemod` down to one walker

- **`project/source-files` and `project/test-files`.** Six tool functions each computed "the
  project's source files" as `(apply append (map (abs-paths root *project-source-paths*)
  collect-sources))`: `docs`, the image fingerprint, `project-check` twice (once privately
  as `source-files`), co-located tests and `changed-files`. They now share one function. The
  image fingerprint still sorts on top. The zero-warning checker gate caught two call sites
  in `--suggest-sigs`/`--fix-sigs` that no test reaches.
- **`codemod` lost 290 of its 603 lines:**
  - The ADR-308 data-first migration (`reorder`, `data-first`, its table and four shape
    heuristics) is finished; every downstream project requires brood ≥ 0.38.
  - `rename-from-file` had no caller.
  - `swap2`'s hand-written s-expression scanner is replaced by a `:swap` mode of the same
    syntax-tree walk `cst-rename` uses. `nest rename --swap` goes through it. The scanner
    matched `(old` inside strings and comments, did not skip `;` inside a form, and split
    `#b"…"` in two. None of that can happen on syntax-tree nodes; tests in `codemod_test`
    cover all three (sabotage-verified).

## 2026-10-15 (later) — one arm rule for both tiers; one wake; one list of a message's children

- **`core::value::select_arm_by`** is now the single multi-arity rule. `Closure::select_arm`
  (tree-walker) and `CompiledClosure::arm_for` (VM) each had their own copy, and a one-sided
  change would have run a different arm at tier 1 only. One sabotage of the shared tie-break
  now reds the precedence test on both tiers at once.
- **linmap:** the rewrite maps ops through the probe's own `linmap_update_op`/`linmap_read_op`
  instead of a hand copy, and `linmap_fold_walk` (a verbatim copy of `pipeline_walk`) is gone.
- **Flags read once:** `BROOD_TRACE_GCBLOCK` was read from the environment on every eval
  entry and exit under debug assertions, and `BROOD_LINMAP` on every top-level form. They and
  five trace flags now go through cached `debug_flags` readers.
- **`mailbox::Wake`:** four wake sites (deliver, L1 delivery, timer, exit) each hand-spelled
  "set `wake_pending`, take the waiter, read `cv_waiters`, drop the lock, wake both". Two of
  them had once done half of it. `MailboxState::take_wake` builds a `#[must_use]` `Wake` under
  the lock, and `fire` runs after it is released. Verified: `wake_both_paths` 5/5,
  `root_exit_signal`/`read_line_parks`/`hosts_receive_fence`, eleven process test files 3×
  each plus once at tier 0, two of them under `BROOD_GC_STRESS`+`VERIFY`, `distribution`
  and `memory_pressure` (the latter twice more at tier 0). Not measured: the deliver path's
  cost — it should be identical (the same steps, inlined), but per `CLAUDE.md` the A/B
  belongs on the benchmark machine.
- **KI-256:** `Message::for_each_child` fixes `message_bytes` ignoring closures.

## 2026-10-15 (later) — one cache directory (KI-257); `Heap::new` is `with_regions`

- **`boot::cache_dir`** replaces seven copies of "where Brood's cache lives". They
  disagreed on a relative `XDG_CACHE_HOME`, and the stdlib image followed the Brood copies
  (KI-257). The Brood side asks the kernel through `%cache-dir`.
- **`Heap::new` is `with_regions`** plus its four GC overrides. The two constructors had
  been written out field by field, about 90 each, differing in six; a field initialised
  differently in one was invisible.
- Two parameters named `seq/keep` (`project-check/project-cache-prune`,
  `editor/buffer/collapse-whitespace`) were left behind by a rename wave that rewrote `keep`.
  They bound fine as locals, which is why nothing noticed. Renamed.

## 2026-10-15 (later) — poisoned locks recover everywhere (KI-258); seven GUI ops are one

- **KI-258:** coverage dropped every hit after a poisoned lock and reported empty. It, the
  GUI backend (33 `.lock().unwrap()`), the wasm host (a hand copy of the recovery) and
  `file/stat`'s name lookup now all recover through `core::sync::lock` or the same
  `PoisonError::into_inner`. No `.lock().unwrap()` is left in the kernel outside tests.
- `gui/focus`, `grab`, `maximize`, `minimize`, `drag-move`, `drag-resize` and `fullscreen`
  were seven copies of "is the window live, headless is a no-op, send the event". They are
  now one `dispatch`.

## 2026-10-15 (later) — one reading of a child's options; one start-up for both binaries

- **`builtins::subprocesses::child_options`** reads `:cwd`/`:env` for `os/spawn`,
  `os/spawn-pty` and `os/cmd`, and `host::subprocess::apply_env` applies it. A nil `:env`
  value removed the variable under `os/cmd` and raised a type error under `os/spawn`. It
  now removes it everywhere, as `os/cmd` documented. A pty child given `TERM nil` keeps
  `TERM` unset instead of getting the `xterm-256color` default. Test in `proc_test`
  (sabotage-verified).
- **`cli_support::install_panic_reporting` and `init_runtime`:** `brood` and `nest` each
  spelled out the backtrace default, the crash dump, the worker cap, the memory limits and
  the GC-config warning.

## 2026-10-15 (later) — the JIT's runtime callbacks are one table

`jit/cranelift.rs` registered 51 `brood_rt_*` callbacks one `builder.symbol` at a time, and
the `BROOD_DUMP_CODE` address dump listed a hand-picked 21 of them. Six were registered and
never called:

- `brood_rt_tick` and `brood_rt_in_capture`, replaced by `tick_n` and the batched poll.
- `brood_rt_push`, replaced by `push_room`; two "bug #2" comments still named it as where
  validation lived.
- `brood_rt_roots_base`, `brood_rt_global_epoch` (replaced by `global_epoch_ptr`) and
  `brood_rt_dispatch_identity`, which served dispatch-speculation phase 2a, abandoned on
  2026-08-29.

They are deleted, with the `Heap::roots_base_ptr` and `tick_capture` re-export only they
used. Registration and the dump now read one `callback_table`.
`every_declared_callback_is_registered_and_used` reads every file under `jit_lower/` and
fails on a name declared but not registered (a link failure on the first arm that uses it)
or registered but never declared. Sabotage: dropping `brood_rt_const_load` from the table
reds it by name.

Also in this batch:

- **The stdlib image decoder** reads through one bounds-checked `take(r, n)`, with `get_u8`
  in place of four inline tag-byte reads, `put_def_site` beside `get_def_site`, and no
  `.unwrap()` in the directory parse.
- **The GC's six local leaf copies** (`flush_string`, `bigint`, `decimal`, `ratio`, `bytes`,
  `rope`) are one `flush_leaf!`, the twin of the runtime side's `flush_rt_leaf!`.
- **`Value::raw_tag`** reads only the discriminant byte. The debug tripwires transmuted the
  whole value to `[i64; 3]`, which also reads the padding after the tag.

**The cranelift 0.136 bump, verified as `jit/cranelift.rs` asks after any Cranelift change:**
the fuzz differential against `target/release-fast/brood` (the verifier off, the build an
armed binary cannot stand in for). Eleven generators, 3400 programs, each under the
tree-walker, VM, VM+JIT and GC stress: 0 divergences, 0 crashes, 0 stale. The other half,
every benchmark row's stderr grepped for CODEGEN-PANICKED, is for the benchmark machine.

## 2026-10-15 (later) — KI-259: aging under another process's isolate snapshot

Running the touched test files under `BROOD_GC_STRESS` turned up `stdimage_test` failing 1
in 6. It was not this session's: the pre-session baseline fails too, once it has a stdlib
image. The first bisect missed that because the image is keyed per commit and none of the
bisect builds had one. The cause was the crash reporter aging the RUNTIME region inside an
`%isolate-discard-loads` window. The snapshot's off-graph handles came back after the
migration, and a stale `*features-loading*` claim made the next `require-one` skip a
module. `rt_collect_block` (KI-6) is per heap and never covered another process. Aging and
freeing now ask the runtime whether any snapshot is outstanding, and a snapshot waits out
an aging cycle in progress. Full account in KI-259.

The tools that found it are worth reusing. `BROOD_SCOPE_DBG` printed the stuck claim at the
restore: its timeout branch now lists every in-flight claim and its owner. One-line traces
in `require-one`'s rare branches did not disturb the timing; the heavy global tracers did.
Correlating the `AGE` lines with failures was exact, 4 for 4.

## 2026-10-15 (later) — one Int×Int table, one ordering, one string-literal rule, one `seqable`

- **`prim2_int_fast` is the VM's single Int×Int table.** `prim_apply`'s integer arm was a
  second copy that differed only in exact `%div`, which the table now handles.
- **`%lt`, `%le`, `%max` and `%min` share one `ordering`.** It returns `None` for a NaN, so
  each caller matches exactly the orderings it means. A "not greater" `<=` would answer
  `(<= nan big)` true; the new `comparison_test` cases use a bignum operand to get past
  every inline fast path (sabotage-verified).
- **`printer::write_string_literal`** is the one escaping rule, and
  `introspect::string_literal` uses it. The old two-character escaper was not wrong, since
  the reader takes raw control characters, but it was a second rule.
- **`seqable` is spelled once (`Ty::SEQABLE`).** `signature_types::seqable` and `sigs.rs`'s
  local `seq` hand-listed the same six tags.
- Stale comments fixed: a `%div` "answers a float" (it answers a `Ratio`), two line
  numbers into a file that no longer exists, and a KI-48 note describing code since replaced.

## 2026-10-15 (later) — one form-scan lexer (KI-260), one tag list, named threads

- **KI-260.** The two native form scans had their own copies of a lexer that knew strings
  and comments but not `|…|` bar symbols. They now share one `lex` that splits tokens as
  the reader does.
- **`Tag::ALL`** is the one list of tags. A `const` assertion pins its order to the
  discriminants, so a reorder fails to compile (sabotage-verified). `types::ALL_TAGS` and
  the `Tag::keyword` cache read it instead of keeping two more copies.
- **`with_string_slot`** reads a RUNTIME generation through `code_gen_ref`'s cache, like
  every other RUNTIME accessor.
- **Scheduler workers are named `brood-worker-N`** on the normal spawn, not only on the
  fallback. Overflow drainers are named too.

## 2026-10-15 (later) — `string/drop-prefix` / `drop-suffix`; two reds from earlier today

- **`string/drop-prefix` and `string/drop-suffix`** replace eleven hand-written
  "`starts-with?` then `substring` past it" strips across std/ (stream, sse, tempo, format,
  docs, complete, test, update-brood, project-release).
- **Two gates went red with `e339d3cc` and are green again.**
  - `renames_test`: the new `project/source-files` reused a name the ledger listed as
    retired. There is now one public name, `project/source-files`; `project-image` sorts
    it privately, and the ledger entry points the other way.
  - `nest check --strict`: the `:swap` rewrite in `codemod.blsp` passed
    `(nth positions 2)`, which may be nil, as an index.
  Neither red was pushed. Before a push now: `make prepush` over the whole unpushed range,
  not just the tests the edited files name.

## 2026-10-15 (later) — one bounded TCP dial; one net-split cleanup

- **`host::dial::dial_tcp`** is the one "resolve, then try each address" policy. Both
  `dist` and `host::net` maintained it, and only dist bounded the DNS lookup (KI-97).
  `tcp-connect` and `tls-connect` now get the 5 s resolve bound as well.
  `split_host_port` takes dist's `host:port` / `[v6]:port` apart, so an IP literal of
  either family still skips the resolver thread.
- **`dist::sever_couplings`** names the net-split cleanup that `fire_nodedown` and the
  duplicate-link eviction both run. The eviction used to re-inline `fire_nodedown`'s last
  two calls.

## 2026-10-15 (later) — receive deadlines are a type, not a convention

`SchedDeadline` (in `process/timer.rs`) replaces the bare `Instant` that carried a
`receive` deadline through the mailbox, the timer heap, `Control::Suspend` and the VM's
`Suspended`.
- Its `Instant` is a private field, and `sched_now` is private to the timer module. Outside
  it, a deadline can only be minted (`after_millis`), tested (`reached`) or waited on
  (`remaining`) against the scheduler clock, so comparing one with real time no longer
  compiles. That comparison is what spins the wasm pump.
- `tests/sched_clock_domain.rs` had read three function bodies as text for that rule. It now
  pins the two privacy facts and the carriers' types. Sabotage: a `pub` field, or a
  `pub(super) sched_now`, each reds one test. Retyping `recv_deadline` back to `Instant`
  fails to compile.
- `scripts/wasm-suite.sh`: 10/10, clock-domain case included.

## 2026-10-15 (later) — one answer to "which values carry a heap handle"

`Value::heap_handle()` returns a `HeapHandle` (`HandleKind` + region, index, epoch, age,
code generation, all read from the one bit layout the `handle!` types share). Several
switches over the variants are now derived from it:
- `is_movable` and `needs_root_slot` (the rooting gate);
- `freeze`'s `handle_key`;
- `gc_runtime`'s `runtime_gen_of`;
- the two debug scans.

The debug scans had drifted. `dbg_value_stale` skipped bigint, decimal, ratio, bytes, range,
seq-view, fn and macro handles, and `dbg_value_oob` knew four slabs, so `BROOD_JIT_VERIFY`
and `BROOD_GC_VERIFY` passed over garbage of those kinds. Guarded by
`handle_debug_tests::the_handle_debug_checks_see_every_kind` (dropping a kind from
`heap_handle` reds it).

`heap_handle` lists its atom variants instead of using `_`, so a new `Value` variant must
say whether it carries a handle; writing that list immediately caught `Pid`.

GC stress plus `BROOD_GC_VERIFY` on seven files are green, as is a 1650-program fuzz
differential.

## 2026-10-15 (later) — `receive_match_timed` has one exit

The scan-and-wait loop moved into `scan_until_answered`, which only says how the receive
ended. `receive_match_timed` then does what every ending shares, once: drop the scan's
operand roots, and clear the persisted capture-mode deadline unless a control signal
(suspend, kill) ended it.

The six hand-written exits had looked different:
- one checked `capture` alone, one added `!is_control()`, and the kill path skipped the
  clear;
- all of these turned out to be the same rule, so behaviour is unchanged.

Nothing tested the rule, so `timer_test` "no way of ending a receive carries its deadline
into the next one" now covers a timeout, a match and a throwing guard. Sabotage: never
clearing reds it, and so does keeping the deadline on any error (only the `:throw` case).
10/10 on repeat; wasm suite 10/10.

## 2026-10-15 (later) — `SlabRef` is gone; accessors return `&T`

`SlabRef` had been a guard wrapper that pinned a RUNTIME generation. Since the per-process
generation cache (`code_gen_ref`) keeps every generation it hands out alive until
`&mut self`, every construction was `SlabRef::direct(&T)` around a raw pointer. Its unsafe
derefs carried SAFETY comments about a guard that no longer existed.

The heap accessors now return plain references, and the compiler checks that each borrow
is tied to `&self`. With them went:
- `rt_slab_ref` and `with_code_gen`, two trivial wrappers around `code_gen_ref`;
- three unsafe blocks;
- the `.map` projection;
- about 30 `&*` re-borrows the wrapper forced (`clippy --fix`, reviewed).

**A red caught here:** `9dc5ff29`'s new `RootsBuf` layout assertion broke the wasm32
build, where a `usize` is four bytes. The JIT that reads +0/+8/+16 exists only on 64-bit,
so the pin is now gated on `target_pointer_width = "64"`. Lesson: a `const` assertion about
a layout must run the wasm check too.

## 2026-10-15 (later) — two bugs (KI-262, KI-263), one formatter walk, the 2024 edition

**KI-262.** `links::notify_peers` delivered a dead process's `[:EXIT]`s after releasing
`LINKS`, so an `unlink` and a zero-timeout flush could both miss one that landed later
(10-93 per 80 000 rounds). That is the link half of KI-213, and the deliveries now happen
under the lock, as the monitor side's do. `process.rs` gains the one lock-order table that
had been spread across ten files' comments. Guard: `tests/unlink_race.rs`.

**The formatter's four child walkers** now read one item list (`layout-items`). The output
is byte-identical on 2,100 inputs: every tracked file as written, unindented, with doubled
newlines, and shifted right. **KI-263** was found doing that: the newline gap restarted at 0
after a trailing comment, so `nest format` deleted the blank line under any line ending in
one, top level included. It restarts at 1 now, and no tracked file changes.

**The 2024 edition.** `unsafe fn` bodies are wrapped, the callbacks are
`#[unsafe(no_mangle)]`, `set_var` carries SAFETY notes, `gen` became `generation`, and 356
nested `if let`s are chains. rustfmt stays on `style_edition = "2021"`, so a whole-tree
reformat can be its own commit. The edition build also found that `dispatch_identity_agrees`
had not linked since its callback was deleted. Clippy never links, so nothing had said so.

Also: one run-cap number; `bench.sh --quick` replaced `quickbench.sh`; `test-light` is the
test target with knobs; tests for twelve public functions nothing referenced; the boot paths
share their head and tail; a refused audio thread is retried; the scaffold's templates are
data.

## 2026-10-16 — merged the other machine's 2026-10-15/16 work; its KI numbers moved

Both machines numbered new known issues from KI-250 on 2026-10-15. This side's KI-250..255
came first, so the other machine's entries were renumbered at the merge: **KI-250 → KI-264,
KI-251 → KI-265, KI-252 → KI-266**. Their commit messages (`9d7c9150`, `985f97fd` and
others) still carry the old numbers. The entries below are theirs, renumbered.

## 2026-10-15 — a native tail call by name caches its head (KI-264)

**Picking `json` apart.** A `json` profile at N=20 000 had two Rust frames that do not belong
on a hot path: `env_get` (4%) and `jit_tier_in_frame` (3%, the per-entry ceremony of VM→native
calls, scattered — no single hot instruction). The counter build said why the first was there:
the global-read cache hit 186 625 times and missed 179 843. Instrumenting the miss path named
three functions, each tail-called BY NAME from native code, and the slot each was looked up in
lay one past the end of the process's table.

**KI-264.** Native code resolves a named tail call's head through the global-read cache, and
passed the call SITE's number as the slot. An arm reserves only as many global-read slots as
its body has reads, so the call-site number pointed past its block (the write refused, every
call an `env_get`) or onto one of its own reads (the two evicting each other). The entry
checks the symbol, so nothing was ever wrong but the cost. An arm now reserves one slot per
call site after its reads, and native code uses that slot. Instructions: `primes` −6.4%,
`supervisor` −1.8%, `latency` −1.8%, `nqueens` −1.1%, `ring` −0.8%; `json` only −0.35% at the
benchmark's N=2 000 (the misses scale with the document; 18 000 a run there).

**KI-265, a test flake found by the gate.** The tree-walker half of `make test-both` failed
`a_leak_is_stopped_and_a_busy_worker_beside_it_is_not` once. With the CPU loaded it failed 7 runs
in 25. The runtime was right every time: the culprit sometimes notices the crossing itself and
raises in itself, which prints nothing on stderr. The test now checks which of the two errors
the culprit died with, and wants the stderr line only from the selector.

## 2026-10-15 (later) — the three `json` leads: stale leaf derivations (KI-266), the slow path's environment walk, a queued callback

**A stale leaf derivation (KI-266).** At `4d1cdbda` `json`'s `skip-ws`, `digits-run` and
`json-object` never got their inlined body: derived at epoch 96, tiered at 113. On the next
commit they did, by timing. The tier-up now re-validates a derivation whose epoch moved — the
closures it spliced, a new runtime count of code-handle relocations, the spliced primitives —
and moves its stamp forward; only a real rebind of a spliced callee recompiles the arm. The
first cut recompiled on any stale stamp and churned (`%match-bind-sym?` 58 times in one run).

**The environment walks.** A `#[track_caller]` tally on `env_get` put ~1M of `json`'s 1.03M (N=20 000)
at one line: `jit_dispatch_call`'s slow path, resolving a named callee native code could not
link. 588 000 were `(nth s i -1)`: `nth`'s 3-argument arm was `(coll i & default)`, and a rest arm
never links, so every call walked the environment, dispatched on the VM and allocated a rest list.
Two fixes: the slow path takes the callee from the call IC the resolve above it has just filled
(every unlinkable named call), and `nth`'s default is a fixed third parameter, as its `sig`
already said — a fourth argument is now an arity error (the checker warned on it before).
Warm, `json` at N=20 000 does ~40 000 walks, from 1.7M this morning. An earlier tally that put
54 000 walks in the tree-walker was a cold run: the first run after a prelude edit re-expands
the prelude into its cache.

**A callback whose compile is queued.** A spinning loop compiles synchronously after 2 048
back-edges spent waiting for its queued compile (`JIT_QUEUED_SYNC_EDGES`); a callback has no
back-edge, so a fold over a cold callback behind a backed-up queue ran entirely interpreted —
`json`'s closing `reduce` at N=20 000, all 19 872 elements. `hof_apply_native` now counts the
calls it declines while queued and compiles on the same threshold: 19 872 → 1 919. The
benchmark rows decline at most 6 273 times a run, so this is for long folds, not the column.

**Measured** against `1d7adbb6` (KI-264 already in), min of 3: `json` −5.1%, `errors` −2.1%,
`primes` −1.6%, `latency` −1.4%, `regex` −1.1%, `base64` −1.0%, `nqueens` and `pipeline` −0.8%.
`errors-deep`, `supervisor`, `wordcount` and `sort` read +0.3–0.5% on six repeats each; their
traces show identical tiering decisions on both binaries (no leaf upgrade runs there), so that is
layout. `ring` is bimodal on both (7.33G / 7.52G).

## 2026-10-15 (evening) — the native → interpreted → native round trip, 23% cheaper

**The lead, and why not the gate.** What is left on `json` is ~470 000 calls a run (N=20 000)
into functions the native compiler refuses (`call-mediated-boxed`: `json-value`, `enc-key`,
`reverse`, …). Compute-frontier §7.1 admitted that class three ways and lost every time, so
the gate stays; the cost to cut is the BOUNDARY. A per-arm tally of interpreted instructions
put ~1.2M short activations (4–22 instructions each) on the row — per-call cost, not loops.
A microbenchmark of `json`'s shape — a native loop calling a refused dispatcher that calls a
native leaf — paid ~3 130 instructions an element against ~495 with the middle function
native.

**Four cuts, all in the plumbing:**
- `jit_dispatch_call`'s slow path ran the callee through `dispatch`, which looked its arm up
  again by closure (a hash lookup, the closure read, its IC block) though the resolve above
  had just found it in the call IC. It now runs that arm with `vm_apply` — what `dispatch`
  ends in — unless a module load has staled it (ADR-366), and resolves the callee value only
  for the `dispatch` fallback. −520, then −45 an element.
- `arm_ic_blocks`, read on every VM frame push, hashed its `uid` with SipHash; past eight arms
  it is a real hash map (`json` runs ~240). Now `SymbolHasher`'s one multiply, as the other
  per-call maps already were. −145.
- The interpreter's call into a native callee cloned the call IC's arm handle a second time;
  every outcome after it returns, so it is moved. Two atomic refcount writes a call — cycles,
  not instructions.
- `jit_tier_in_frame` tested the global xcall switch (a call) on every native entry before the
  arm's cached "has a non-tail named call" answer, which ends the test for a leaf. −33.

Round trip 9.80G → 7.56G instructions (−23%) and 3.4G → 2.6G cycles at N=3M. Rows against
`985f97fd`: `nqueens` −3.9%, `nbody` −2.0%, `json` −1.4%, `regex` −1.3%, `startup` −1.0%;
`spawn` and `latency` read +1.3% / +1.0% once and overlap fully on six repeats each (noise).
What remains of the round trip is the trampoline itself (`vm_run_bc` per nested entry, the
frame's arm refcount) and `jit_tier_in_frame`'s entry ceremony — handoff's "the rest of the
VM call", structural.

## 2026-10-15 (night) — spike: resuming native code through the driver does not pay for compute

**The question.** Should a native → interpreted call stop nesting an interpreter run and
instead return to the driver, which runs the callee as an ordinary frame and re-enters the
caller's native code at the call site (a resumable native frame, BEAM's model)? The same
design is what a compiled `receive` loop needs, so the spike asked the compute half first:
is resuming through the driver cheaper than nesting?

**Measured instead of built.** The new path is the sum of two costs that exist today, so they
bound it. Microbenchmarks (instructions a call, min of 3, N 1M vs 3M, an interpreted driver
kept off the JIT by a `binding`):

| path | instructions |
|---|---|
| interpreted → interpreted call, callee body included (`BROOD_TIER=1`) | 768 |
| interpreted → native call | 640 |
| today's nested native → interpreted crossing, callee body included | ~1 180 |

The resumed crossing is an exit from native code (~50), the driver running the callee
(~768), and a re-entry at the call site — at least the native-entry ceremony (~200, from the
computed-call work), plus reloading what was live across the call: carry variables, the
per-slot register caches and the operand stack's block parameters, all of which the lowering
keeps in SSA values and would have to write to the frame before every such exit. ~1 020–1 300
against ~1 180: break-even. After the day's cuts the nesting is no longer the cost; the two
per-call constants are, and the redesign pays both.

**What follows.** Not built, for compute. The compute lever is the per-call constants
themselves — 768 for an interpreted call, 640 into native code, ~410 native to native, where
BEAM pays tens. The resumable native frame stays the design for compiled `receive` loops (it
is the only way a native frame can park), and it is judged on the process rows, not on
these; it will want the cheaper entry either way.

## 2026-10-15 (late) — the call into native code, 32% cheaper: an arm that has settled skips the tier-up

**The constant.** The spike left three per-call constants as the compute lever; this takes the
middle one, an interpreted caller entering native code (640 instructions, the
`drv_base`/`drv_leaf` microbenchmarks: an interpreted driver kept off the JIT by a `binding`,
with and without a call to a native leaf). Precise sampling (`instructions:upp`; plain
`perf record` here is capped at 1 000 samples/s and skids onto the instruction after an
atomic) split it into `jit_tier_in_frame` ~227, the call site in `exec_chunk`, `enter_native`'s
ceremony, `native_feedback` and the stack stamp.

**Cuts:**
- `shared_published.swap(true)` ran on EVERY native entry of an arm without an installed
  inlined body — a locked RMW on a `CompiledArm` all of a runtime's processes share, so each
  entry wrote that cache line and a fan-out bounced it between cores. Now a plain load first;
  `inline_queued.swap` the same (it ran on every entry while a deferred compile waited — a
  spawn storm's whole run).
- An arm that has nothing left for the tier-up to do records the code pointer it SETTLED on
  (`CompiledArm::settled_code`); `jit_tier_in_frame` is now an inline fast path for that
  pointer — the drain, the ceiling, the epoch and its re-load, the frame size, then the entry —
  and everything else is `jit_tier_in_frame_slow`. Keyed on the pointer, so every writer of
  `jit_code` voids it unawares; `reset_native_state` clears it (the same pointer can be
  re-adopted from the shared cache at an unchanged epoch). Only a BAILED upgrade slot counts as
  finished: a null one may be a deferred compile in flight, so the xcall re-lowering's swap
  (which nulls the slot) settles its own pointer. Skipping the slow path can only miss an
  upgrade, never run wrong code.
- `enter_native` re-rooted an env its driver caller had already rooted (`EntryEnv::Rooted`).
- The direct call site copied the args into a `SmallVec` and pushed them back to the slots
  they came from; now they stay put (a staged head shifts them down one, in place).
- `native_feedback` returns inline on a clean exit with nothing blocked and no deopt streak.

Not taken: caching the stack stamp per thread (~25 instructions) — lowering runs under
`stacker::maybe_grow`, so a macro can enter native code at depth 0 on a heap segment whose
limit a cached value would get wrong, silently switching the overflow guard off.

**Measured.** The call: 636 → 434 instructions (−32%), ~148 → ~84 cycles (−43%). Rows,
instructions against `d35af1a1` (a proper `target/ab` baseline — a hand-copied binary booted
without its stdlib image and read −30..−80% everywhere): `json` −1.6% in each of its two
modes (30 runs a side), `nbody` −2.9%, `regex` −3.2%, `spawn` −3.3%, `latency` ~−2% (10 a
side), `persistent-map` −1.5%, `bintree` −1.3%, the rest within ±1%. One `json` run of 40
read 2 896M (the first of its batch) — closed 2026-10-16: the first run after a rebuild boots
the prelude from source (a new build id), not a tiering failure.

**Guards.** `an_interpreted_callers_native_callees_settle_on_their_code` (Rust, debug build)
and `stress/jit_settle_test.blsp` (release, `make check-stress`; the debug build never
re-lowers) via the new `:settled` key of `%jit-arm-state`. Sabotaged: settling while an
upgrade is pending reds `an_unrelated_def_does_not_cost_a_leaf_derivation_its_inlined_body`;
dropping the swap's settle reds the stress test. Gates: `test-both` 1882/1882 both engines,
fuzz (tier_transition, metamorphic, pipeline, arithmetic × 300, stale=0) clean, `gcstress`
clean.

## 2026-10-16 — the interpreted call, 10% cheaper; three cuts measured and NOT taken

**The constant.** The interpreted → interpreted call (the `drv_*` microbenchmarks at
`BROOD_TIER=1`): ~771 instructions. Precise sampling: ~325 in `vm_run_bc`'s frame push/pop and
loop top, the rest spread through `exec_chunk`'s `Call`/`Return` handling and its two entries
per call — the shape the 2026-09-25 entry found, whose structural fix (frame push/pop inside
`exec_chunk`) regressed `ring`/`ackermann` on L1i and stays dropped.

**Shipped — two cuts that are equivalent by construction:**
- `fill_captures` was `#[inline]` but too big to inline, so every `CallInPlace` frame paid a
  call to find an arm with no captures; the fill is split out and the emptiness test inlines.
- A self-tail loop's frame reset (`reset_frame_keep_captures`) tested every slot for being a
  capture; an arm with none (nearly every loop) truncates and nil-fills in one write. It
  showed because a call in a loop body gives the loop's frame spill slots (`drive`: 6 slots
  against 2), all reset per iteration.

Call 771 → 691 instructions (−10%). Rows, instructions, median of 5 against `fec73f3f`: at
`BROOD_TIER=1` `fib` −3.4%, `spawn` −3.3%, `bintree` −2.6%, `json` −2.4%, `latency` −2.2%,
`ackermann` −1.9%, `nbody` −1.8%, every row ≤ 0 but `sort` (+0.04%); at the default ceiling
`ring` −3.3%, `nbody` −1.3%, `supervisor` −0.7%, `json` (20 a side, both modes) flat.

**Measured and dropped:**
- **One thread-local for the quantum state** (`REDUCTIONS` + `OWED`, each access an
  out-of-line accessor call here): 691 → 688. Not worth the reshuffle.
- **The fast-link table's base from a heap mirror** instead of `brood_rt_fastlink_base` (a
  callback per native→native call site): −4 instructions on a ~255-instruction call, inside
  the noise — against five sync points whose one omission reads freed memory.
- **`brood_rt_push_room` inlined in IR** (bump `RootsBuf`'s length when it has the room, call
  only to grow): −9 instructions and −3% cycles per native→native call, `bintree` −2.9% — and
  `json` +2.5% in both its modes (20 runs a side; bisected to this change alone). The extra
  blocks and the join parameter at every call site cost the big call-heavy arms more than the
  callback did. A regression on a row is stop-the-world, so it is not in.

**Where the native→native call goes** (a self-tail loop calling a recursive — so unspliced —
callee, after the xcall re-lowering installs: ~255 instructions): the caller's in-IR sequence
~139 (the fast-link guards, nine `JitCallCtx` stores, the gateway bump, the checkpoint journal
written after the call), the callee's boxed entry wrapper ~39 around its i64 worker ~21. The
structural lever is a direct i64-convention call into the worker when the caller knows its
operands are ints, skipping the wrapper both ways.

**Two things learned about measuring here.** (1) The first run of a freshly built binary boots
the prelude from source (a new build id), +1.6G instructions on `json`: yesterday's
"unreproducible 2 896M outlier" was exactly that, so it is closed — discard the first run after
any build. (2) `cli::stdimage_reporting` went red in the tree-walker half of a `test-both`
because a `cargo build` (debug) in another shell relinked `target/debug/brood` between the
test's two steps; reproduced on demand by rebuilding mid-test. Never build the debug profile
while a suite runs.

## 2026-10-16 (later) — the merged tree's first full `test-both`: four reds, all from moves the other machine could not run

The handoff asked for `make test-both` here first: the other machine's 62 commits (the 2024
edition, the file splits, KI-256..263) were verified there with targeted tests only. Both
engines went red on the same four, so each had one shared cause — none a runtime defect:

- `deterministic_unary_is_complete` read `DETERMINISTIC_UNARY` out of `types/check/guards.rs`;
  `a93e3f08` moved the list to `known_ops.rs`. The test reads the new file.
- `devirt_differential` ran `brood --test tests/ability_test.blsp`, which `c4b9a8ff` made
  `(:use suite-helpers)`; `nest test` finds that through `project.blsp`'s `:source-paths`, a
  bare `brood --test` reads no project. The differential now loads
  `tests/support/suite-helpers.blsp` first in the same run.
- `temp_purge_coverage_test`: `e2962c19` gave `complete_test.blsp` a `brood-complete-`
  fixture without the load-time `purge-stale-temp` (KI-30). Added.
- `suite_test` "runtime errors" pinned `rem:` and `subvec:`, the names KI-255 (`e2962c19`)
  retired because no such functions exist; the messages now say `%rem:` and `seq/subvec:`,
  and the pins follow.

And CI's red on `fec73f3f` was a flake, not that change: `process_limit_test` "the bound is
ONE-SHOT per arming" timed out on its first try under load and passed the retry. A test race,
KI-267 — the worker announced `:ready` before entering its `try`, so a preemption between the
two let the flood trip `E0046` outside the handler; reproduced on demand with a `sleep` in that
gap, fixed in all four places the file had the ordering.

## 2026-10-16 — a closure copied between processes keeps its compiled code (KI-268)

The v0.39.0 release train's downstream check found store-postgres's 200-checkout stress test
hanging on main; v0.38.0 passed it. Bisected to `67a540e1` (ADR-405, `spawn` copies captured
values), then pinned by gdb and a scheduler trace. A closure that crosses into another process
of the same runtime was rebuilt there as LOCAL data, which never compiles (`cache_key` needs a
shared body form), so it tree-walked, and every `receive` in it held an OS thread. 200 clients
meant ~190 overflow threads; when the OS refused the next one, the stranded work was dropped
silently. The send path had the same flaw since long before 0.38 (a repro hung v0.38.0 3/5).

A same-runtime copy now carries the closure's shared arms by handle, pinned like `FnShared`, so
the receiver runs compiled code that parks. A refused overflow thread is reported once and
retried when a blocked thread wakes. Guard: `closure_copy_stays_compiled.rs`
(`:peak-threads <= :workers`, red under sabotage). Release held until this was in.

## 2026-10-16 (evening) — `exec_value` retired: the bytecode loop is the VM's only executor

`exec_value` walked the compiled `Node` tree for two callers the bytecode never served — top-level
forms and `&optional` defaults — and reimplemented `If`/`Do`/`LetBind`/`Call`/`Prim2`/`TryCatch`
beside `exec_chunk`, so every node-level change had to be made twice (its call-site cache handling
had drifted once already). Both callers now run bytecode, and it is gone, `exec_call` with it; the
prim helpers it shared a file with are `eval/compile/prim_exec.rs`.

- **Top-level forms.** `run` compiles the form's node to a chunk (every node kind already had an
  emission — `compile_chunk` never declined), wraps it in a throwaway zero-parameter arm (`BAILED`,
  so it never tiers: it runs once) and calls `vm_apply`, the nested-run entry — so the frame, the
  IC-block install and the caller's restore, the live-arm registration and the native-stack guard
  are the driver's, as for any call. The KI-245 guard stays: a nested driver cannot capture either.
- **`&optional` defaults.** A real default is now a prologue in its arm's own bytecode:
  `(if <missing> (set <slot> <default>) nil)` per defaulted optional, left to right, ahead of the
  body (which keeps its tail position). "Missing" is a hidden flag slot above the body's slots that
  `push_frame` sets to `true` — `CompiledArm::optional_flags` replaces `optional_defaults`. A
  truthiness test, so no operator a program could rebind is involved; `push_frame` is the only
  frame builder an arm with optionals reaches (every other one refuses `noptional != 0`).
- **Evaluation order at top level.** `exec_value` resolved a call's operator before its arguments
  (the tree-walker's order); the bytecode elides a free-global head and resolves it AFTER them
  unless an argument can run user code (KI-19). Inside function bodies that has always been so,
  but at top level it would have made `(definterface Foo)` at a REPL report `Foo` instead of
  `definterface` and its hint — `hints_name_only_features_that_exist` caught it. A top-level
  form's scope now stages every call (`Scope::stage_calls`), and a staged head's `GlobalIc`, which
  carries no position, reports its call's position from `CompiledArm::staged_head_pos` on a miss
  (`unbound_head_in_tail_position_carries_call_pos` caught that one). Both were red before their
  fix.

Guard: `tests/optional_defaults_test.blsp` — defaults that read earlier parameters, bind locals,
call out, raise; one that must NOT run when supplied (an explicit `nil` included); a 100 000-deep
tail loop through an optional accumulator; captures; across processes. Sabotage: flagging every
optional instead of the missing ones reds six of its eight tests.

**Measured** (instructions, against `b98cda3c`). A loop of 20 000 `reflect/eval-string` calls —
the per-form setup the change could cost — −0.8% at the default ceiling, +2.0% at
`BROOD_TIER=1`; `startup` +0.07%; the rows within ±0.5% but `supervisor`, +1.4% in
`release-fast` at both ceilings (10 runs a side). None of `supervisor`'s hot paths has a real
default, and the profile difference was inlining moving (`jit_dispatch_call` into
`brood_rt_call_slow`, `prim_apply` out of line) — `release-fast` has 16 codegen units, and a
new module (`prim_exec`) re-partitions them. In `release-lean`, the shipped build (fat LTO, one
unit), `supervisor` reads 4 731M against 4 734M, `json` identical in both modes, `nqueens` −0.3%.

**And a flake found by the gates, fixed (KI-269 — filed as KI-268 in its commit message; the other machine's KI-268 landed first and this one was renumbered at the merge).** `stream_test`'s loopback `->socket` test
timed out once in the VM half; 5/120 at 8-way load on `b98cda3c` too, so not this change.
`stream/socket->` let its stream process claim the socket itself, and a connected socket reads
from the start, so a fast peer's data and close could land in the caller's mailbox first and be
lost. The other machine fixed the same race the same morning (`c9092371`: the caller forwards what it
holds once the producer owns the socket, and a no-requester error stays answerable); the merge kept
that version and its test. 0/120 under the load repro.

## 2026-10-16 (night) — two things the `exec_value` retirement exposed, found by bedit

CI's bedit smoke went red on the merged tree (`02271160`, BEDIT_REF moved to bedit 0.5.0): 16 of
`magit_test`'s 40 raised `%map-assoc: expected map, got nil` in `git-git`, whose signature is
`(root args &optional (opts {}))`. Two separate bugs; v0.39.0 (`477d97c1`) has neither, because
both need the retirement.

- **Native entry cleared the missing-optional flags.** `emit_nil_locals` nils every slot above the
  parameters and captures at native entry, so a stale handle from an earlier frame is never
  traced. The new flag slots live up there, so `push_frame` set a flag, native entry cleared it,
  and the prologue skipped the default. The deopt that followed re-ran the same cleared frame, so
  the bytecode failed the same way. Only on the native tier, and only once the arm had lowered,
  which is why bedit's later tests failed and its first ones passed. The flag slots are now
  skipped: an arm with optionals is entered only through `push_frame` (a direct native call
  refuses `noptional != 0`), and that zero-fills the frame, so a flag it did not set is nil.
  Guard: `optional_defaults_test.blsp` settles an `&optional` arm to `:native` and calls it with
  and without the optional (no deopts). It goes red when the flags are cleared again.
- **A top-level multi-pair `assoc` lost its head.** The September unroll of
  `(assoc m k1 v1 k2 v2 …)` built its single-pair calls with a `Global` head and no call site.
  In an arm, `lower_map_prim3` turns them into `Prim3` before emit. A top-level form never meets
  that pass, and `exec_value` used to resolve a site-less head by name. Since the retirement those
  calls are emitted as bytecode, where a named head is resolved only through its site: a debug
  build panicked in `emit`, and a release build ran the wrong code. Each unrolled call now gets a
  site of its own, as `%set` always did. Guard: `assoc_unroll_test.blsp` evaluates a multi-pair
  `assoc` through `reflect/eval-string` (red with the site removed).

## 2026-10-16 (night) — the regression bedit caught: native code wiped the `&optional` flag (KI-270)

CI's downstream smoke went red on `02271160`: bedit's `git-git`, `(&optional (opts {}))`, saw
`opts` as nil. The `exec_value` retirement's default prologue reads a "missing" flag slot that
`push_frame` sets above the body's slots — and the JIT's entry nil-fills everything above the
parameters, flag included. Every local gate passed because no test called a NATIVE arm without
its optional; the new test does, and reproduced bedit's error exactly before the fix. Both machines fixed it the same hour; the merge kept the other machine's `emit_nil_locals` skip
and test (`0fb04653`, which also fixed a top-level multi-pair `assoc` built without call sites)
and this side's prologue clearing a flag as its default lands. bedit at
`BEDIT_REF`, locally with a grammar-enabled `nest`: 2463/2464. The lesson is the CLAUDE.md one,
paid again: a guard has to reach the shape a caller reaches, and "tiers 0–2 pass" is not that
when the tier the arm actually ran on was never entered.


## 2026-10-16 — a plain `assert`, the rustfmt 2024 reformat, the concurrency teaching note

- **`assert`** is a prelude macro beside `error`: `(assert test message…)` raises `"assert
  failed: <form>"` (plus `": "` and the message parts, joined like `error`'s) when `test` is
  falsy, and returns `test`'s value otherwise. The form's text is baked in at expansion, the
  test is evaluated once, the message only on failure. Always on: no build strips it, because a
  knob that turns invariants off is a knob every user pays for (ADR-011). Guard:
  `tests/assert_test.blsp`, including a worker whose failed assert is its exit reason; red under
  sabotage (test evaluated twice, value dropped, message joined bare).
- **rustfmt `style_edition = "2024"`**, the reformat held back when the workspace moved to the
  2024 edition: `cargo fmt` only, its own commit. It needed two passes to settle (one rewrapped
  `return Err(…)` in the reader only on the second).
- **The concurrency teaching note** from the 2026-05-30 Game of Life findings: a fan-out per
  generation loses to one process (the board crosses heaps twice a step and the merge is
  serial); tiling with halo exchange is the shape that can win. It is in
  `docs/brood-for-claude.md` beside the worker pool, so every scaffolded project carries it, and
  `examples/life_bands.blsp` is the worked version — a process per band of rows for the whole
  run, neighbours swapping edge rows with the generation pinned in the `receive`. The run
  `assert`s it equals the serial step (verified at 1, 3, 4, 5 and 24 bands; red with the halo
  from above dropped).
- **Docstring examples for the bare core:** 23 of the 42 bare names that had a docstring but no
  example now carry executed `form → result` lines — the process primitives (`self`, `send`,
  `receive`, `spawn`, `spawn-link`, `monitor`, `demonitor`, `exit`, `ref`), `throw`, `match`,
  `case`, `binding`, `check-allow`, `satisfies?`, `->string`, `inspect`, `gensym`, `arglist`,
  `doc`, `bytes`, `macroexpand-1`. Each is self-contained (a process example sends to `self` or
  waits on its own monitor, with a `:doc-` tag no other message can match). Left without: the
  defining forms (an executed `defn` example would define a global in the test process),
  `link`/`unlink` (a broken `unlink` would kill the doctest process after its case had passed),
  and `compare-to`, which has no method for a built-in kind. `spawn`'s docstring named the
  monitor message `[:down pid …]`; it is `[:down ref pid reason]`.
- **Stale docs fixed on the way:** `failure`'s docstring and the value-kinds table in
  `language.md` still said a failure is FALSY (it is truthy, deliberately — the Truthiness
  section says why); `audit/report`'s doc example printed the whole surface audit into every
  doc-examples run, and now captures it with `with-out-str`.

## 2026-10-17 — `receive` matches simple patterns natively (ADR-409)

**Where a message hop goes.** The handoff's next thread was "receive loops run interpreted".
Measured first: on `ring` a hop is ~9 500 instructions, but the interpreter executes only 21
bytecode instructions of it — compiling the loop would save little (the 2026-10-15 spike's
conclusion again). The cost that stood out was the matcher: a closure built per `receive` and
called from the kernel per scanned message. So this came first, and the compiled receive loop
second.

**What it took, measured three times.** The spec as a vector literal built at the call site:
slower (`pingpong` +12%) — four allocations a `receive` against a cached closure. As a quoted
constant: `spawn` +47% — and the cause was not the constant: the new binary's stdlib image was
stale after the `std/` edit, so every run loaded `std/` from source (`(stdimage/status)` said
`:stale`); `nest stdimage` fixed the measurement, and the counters had shown it (base and new
disagreed on `tw_defer` by 180×). Then a keyword key (an arm with a heap constant is not shared
across processes, which matters regardless) resolved per CANDIDATE: `ring` +7%, `pingpong`
+5.6% — a SipHash lookup and an `Rc` clone two or three times a hop. Resolved once per
`receive` with the symbol hasher: `supervisor` −7.9%, `spawn-live` −7.8%, `pingpong` −3.7%,
`spawn` −2.4%, `ring` −2%. The measurement lesson, in `benchmarking.md` already and paid for
again: after a `std/` edit, `nest stdimage` (or `make ab`) before any row.

Gates: `test-both` 1899/1899 on both engines (after one fix — the new helpers' `;; ---- … ----`
comment was a section header to the doc catalogue, which then filed `receive` and `sleep` under
`:other`), `gcstress`, fuzz ×5 (`match` included), `check-stress`, bedit 2463/2464 (the known
Elixir-dev header). `latency-gate` failed once right after a release build (load average 6, worst
p99 673 µs) and passed on a quiet machine; base and new interleaved, 3 runs each twice: worst p99
84/82 µs against 84/80 µs — indistinguishable.


## 2026-10-16 (late) — a receive loop runs in native code (ADR-410)

Item B of the two receive items (A was ADR-409). The `%receive` fence kept every
receive-bearing arm off the JIT, so every server loop ran on the bytecode VM. An arm whose one
`%receive` comes before anything with an effect, `(defn loop (state) (receive …))`, now goes
native. When its native `%receive` parks, the driver captures the frame at ip 0, and the wake
re-tiers into the native loop. Only an activation a top-level driver entered may run such an arm
natively; every nested entry runs it on the VM, where the `receive` blocks as before.

**Measured** (instructions, against `55e6f3c4`, both stdlib images live, median of 5): `ring`
6 919M → 5 578M (−19.4%), `pingpong` −7.3%. `supervisor`, `spawn`, `spawn-live`, `json`, `fib`
and `nqueens` are flat.

**What the sabotage pass changed.** Three of the new guards first survived their own removal:

- The interpreter's direct native call. A one-shot non-tail call never warms the inline cache
  it needs, so the test now runs forty sessions from one hot call site.
- The `fold` fast frame. Only `fold` and the mailbox matcher use it, so the test needs a
  two-argument loop that is native as a process loop *and* used as a `fold` step.
- The native links. These are reachable only from a caller lowered before its process compiled
  the loop. `late-sessions` runs 300 000 iterations without calling the loop, and then calls it.

Writing that last test showed that a nested driver tiered the arm in, blocked, and latched the
shared arm `:bailed` for every process. The refusal is now one check in `jit_tier_in_frame`
(`receive_at_head && !unwind`), which replaced the separate guards I had written in `vm_run_bc`
and in `dispatch`'s fast path. I dropped two guards that could not fail: the VM park rewinding
to ip 0, and the `dispatch` refusal, which the central check covers.

## 2026-10-17 — ADR-410 generalised, measured slower, narrowed back

`060ed7c0` let any receive-bearing arm that journals deopt checkpoints park in its own native
`%receive`. The park resumed the frame on the VM at the last checkpoint, so a `send` before the
`receive` never repeated. That admitted send-then-receive loops (`pingpong`'s `ping`,
`gen/call`) and two receives in one arm, and it was correct: the resume-at-ip-0 and
dropped-operands sabotages each reddened the new tests.

It was slower. Against `5e8aba17` (instructions, 10 runs a side): `pingpong` +1.9%,
`supervisor` +4.8%, `ring` within noise. A send-then-receive loop parks on nearly every
iteration, and each park cost a native entry, an exit, a VM resume of the rest of the iteration
and a re-tier on the back edge. The native stretch in between was little more than the `send`.
The code is back to `5e8aba17`'s receive-at-head gate (`pingpong` +0.5%, `supervisor` −1.3%
against it, which is drift). The three new tests stay for their answers, without their
`:native` checks. Making this shape pay needs the wake to re-enter native code at the
`receive` itself, a native continuation.


## 2026-10-17 — hot reload and observability checked against native receive loops

Question from review: did the speedups (ADR-409/410, the JIT call-path work) cost hot reload or
runtime observability? It was checked, not assumed. A native receive loop (`:native` per
`%jit-arm-state`) picks up a redefinition from its next iteration, whether it is parked in
`receive` or busy draining 200 000 queued messages with no park. A redefinition makes the native
loop hand over to the VM, whose `SelfCall` guard (ADR-013) re-resolves the name. `proc/info` reads
it (`:waiting`, mailbox, advancing reductions). `trace-fn` sees its calls, and `untrace-fn` puts
it back. None of this was covered for *native* loops before; the existing hot-reload tests use
interpreted ones. It is now: `tests/hot_reload_test.blsp` "hot reload reaches native receive
loops", 4 tests, green on tiers 0–2 and 24/24 under 8-way load. Sabotaging the `SelfCall` guard
reddens three of them on both VM tiers.

## 2026-10-17 — a receive loop that parks on every message never went native (fixed)

Found by GC stress on `tests/receive_loop_jit_test.blsp`, where `acc-loop` read `:untried` after
3 000 messages in one run of four. The tier threshold counts entries at ip 0, plus a bonus for a
back-edge exit. An interpreted receive loop parked at its `%receive` call, mid-arm, so a loop
that parks on every message (a lightly loaded server: the common case) counted one back edge per
wake and never got hot. The bursts in the tests and benchmarks hid it. The fix is the
interpreter's park at ip 0 for receive-at-head arms, which yesterday's entry removed as "a guard
that could not fail". It could; the traffic just never exercised it. Guard: "a loop that parks
on EVERY message still goes native", with a loop function no other test warms. It is red
without the fix (`:untried` after 400 one-at-a-time messages). Also added: a native loop killed
while parked reports `:killed` to its monitor, and one whose linked peer dies exits too.
Tiers 0–2 green; GC stress ×3 clean on both new test files.

## 2026-10-17 — every design promise has a guard; two of them were broken (KI-272, KI-273)

An audit mapped what the design promises (CLAUDE.md, ADR-013/026/042/123..126/381, the wire
format) to the tests that hold it, and found eight promises nothing guarded. Each now has a
guard, sabotage-verified unless noted. Writing them found two real bugs.

- **No data-mutation primitive (ADR-026).** `crates/lisp/tests/natives_never_mutate.rs` calls
  every native the image binds (~370, effectful ones skipped by EXACT name) with freshly built
  aggregates in 1-3 argument shapes, ~3 200 calls returning, and fails if any argument prints
  differently afterwards. A temporary `%vec-poke` that wrote into its vector went red, named.
  Beside it, the forbidden names (`set!`, `while`, `transient`, `assoc!`, `atom`, …) must stay
  unbound as globals and as special forms.
- **The reload diagnostics (ADR-042 §5).** `crates/cli/tests/reload_diagnostics.rs`: a macro
  redefinition and an arity change each print their `[reload]` line on tiers 0-2, in one file and
  through `system/reload-defs`; a first definition, a same-arity rebinding and a value `def` in a
  loop stay silent; `BROOD_NO_RELOAD_DIAG` silences both. Red with the macro line disabled.
- **The checker follows the live image (ADR-123..126).** `tests/checker_follows_live_image_test`:
  after a redefinition, an arity, a redeclared `sig` and a value global each flip which use
  warns, including a redefinition made in another process and a check made in one; and a
  redefinition the declared `sig` contradicts still lands. Construction, not sabotage: every
  flip is asserted in both directions, so a stale checker and a silent one both fail.
- **Exact numbers across processes and nodes.** `tests/exact_numbers_across_processes_test`
  (BigInt/Decimal/Ratio: built in a worker, round-tripped, fan-in, a shared global read by 50
  processes, a literal pattern in a selective receive) and a wire round trip into a second heap
  (`dist/wire_tests.rs`). Red with decimals decoded as strings. The literal pattern found
  **KI-272**: `receive` on `19.99M`, `1/2`, a bignum or `'(1 2)` raised "expected a map" on
  every message. ADR-409's macro admitted literals the kernel's spec parser had no form for.
  `%receive-spec-key` now refuses a spec it cannot parse, so the macro keeps the closure.
- **Contracts across processes.** `contract_test` "contracts across processes": a `sig!` in a
  worker, forty workers at once, an armed `sig` in a worker exactly when armed on the runner, and
  a worker killed by a contract error. That last one found **KI-273**: the exit reason dropped
  what the process threw (`:kind :user`, blame flattened), against language.md's "the map a
  `catch` binds". `message::error_reason_in` now builds it from `caught_value`.
  `spawn_monitor_test` "the reason is what a catch would bind" pins it.
- **Cross-node `gen/call`.** `cli::distribution gen_call_round_trips_across_nodes_and_reports_a_remote_crash`:
  exact numbers through a remote server, its crash reason read through a monitor across the
  wire, then `:noproc`. By pid: `monitor` refuses a remote `{:name :node}` address, so `gen/call`
  by name across nodes does not work. Recorded in ROADMAP "Still open" for the user to decide.
- **Contracts armed over the whole suite.** Armed is what `nest test` users get, but the cargo
  wrapper runs unarmed. Armed costs twice the CPU (251 s against 126 s; 128 s against 103 s
  wall), and `make test` already saturates 12 cores (`not binary(suite)` alone reads 962%
  CPU), so it would breach the standard suite's 5-minute budget. It is a gate of its own
  instead: `make test-contracts`, CI job `contracts-armed`, in `green-all`. The suite is green
  armed, 6889/6889.
- **GC stress over everything.** `make gcstress-all`: all 291 test files under
  GC_STRESS + GC_VERIFY via `nest test` (most files need tests/support/), ~80 s at -P 6. All
  clean. Nightly and `green-all`.
- **`tier-audit` nightly; `latency-gate` in `green-all`.** The tier audit is a value gate, so it
  runs on a shared runner. The nightly job asserts the benchmark checkout and a native tier
  first, because the script passes on a missing checkout and on a JIT-less binary. The latency
  gate measures p99, which a shared runner cannot hold (docs/benchmarking.md), so it stays local.

## 2026-10-17 (later) — `gen/call` by name across nodes (ADR-411); CI and nightly back to green

- **ADR-411:** `monitor` takes a remote `{:name :node}` address. A `Frame::MonitorName` is
  resolved by the peer on arrival; an unregistered name fires `:noproc` through the same
  placeholder pid the local path uses. `gen/call` by name across nodes works, with its crash
  reason and a later `:noproc`. Guard extended:
  `gen_call_round_trips_across_nodes_by_pid_and_by_name`, red with the receiver ignoring the name.
- **CI `cargo-deny`** had gone red on a dependency yanked upstream (`inout` 0.2.3 -> 0.2.4).
- **Nightly ASAN** had been red for three nights: `callback_table_tests` scanned source text and
  so expected the two `brood_rt_dbg_*` callbacks, which exist only under `debug_assertions`, in
  a build without them. Its abort hid every later ASAN binary. The other nightly red of those
  three nights (`stream_test` "->socket drains a stream", a reader timeout) was KI-269 on a
  commit before its fix.
- `Value::raw_tag`, read only by debug tripwires and a test, is compiled only there: release
  builds no longer warn about it.
- **KI-271 fixed.** The distribution test's 20 s listen stall was the run-check verdict cache:
  swept on every miss (108 173 entries, 108 197 `statx` per miss), and concurrent sweeps deleting
  week-old batches held every booting `brood` in `D:open_last_lookups`. The sweep now runs hourly,
  claimed atomically by one process, and bounds the cache to 10 000 entries. Reproduced on demand
  before (30 concurrent misses, max 40.6 s with a racy first fix) and measured after (max 1.72 s
  for the one-time trim, 0.49 s mean steady). No KI is open.
- **Three more nightly-ASAN reds** sat behind the callback-table abort, all test bugs of the
  single-process `cargo test` the ASAN job uses: three JIT tier-up waits gave a flat 60 s while
  ~1 000 sibling tests shared the one background compiler (`wait_for_tier` now stops the clock
  while a compile is queued), and the KI-248 allocator test stored a real 512 MiB hard limit, so
  a sibling holding 1 GiB aborted the process (it drives the pure `hard_verdict` now).
- **A review of the day's kernel diff** (read-only, by a subagent) found no defect and two
  hardenings, both done: the sweep claim is a `try_lock` on the stamp (a rename claim could let
  two sweep), and `error_reason_in` runs under `catch_unwind`, falling back to `error_reason`, so
  a panic reading a thrown value cannot take a scheduler worker with it.
- **KI-274 fixed** (found by the review's second `test-both`): a native self-loop that hoisted no
  global had no back-edge epoch guard, so it never saw a `def` of itself while it ran — a busy
  receive loop or a compute loop kept its old body until it left native code. Every `SelfCall`
  arm now snapshots the epoch at entry; the busy hot-reload test is deterministic now, and a
  compute-loop case joins it.
- **`any?`'s no-deopt guard** (`jit_eq_join_test`) counted the shared prelude arm's lifetime
  deopts, which siblings legitimately add (the nightly ASAN run read 1); it counts its own drive.
- **KI-275 fixed:** an edited `include-str` file did not make the project image stale. Found
  closing the last bedit smoke failure, which was two Elixir 1.21-dev differences in bedit's
  agent (an ANSI-coloured `dbg` header; checker chunk `v9`). Both are fixed in bedit, and the
  smoke is 2464/2464 locally; brood's pin moves with it.
- **KI-276 fixed:** the project image was not keyed by contract mode, so an image `nest test`
  wrote (armed) was served to the downstream smoke's unarmed run, and bedit's M-r test read
  line 0. Found by arming a diagnostic in that test (bedit `d50442f`) after it failed once in ~42
  runs: the failure was deterministic per checkout, not random. `fingerprint-of` includes the mode.
- **`fuzzy_ranker_test`'s burst test** raced on a 2-core runner under GC stress (nightly
  `gcstress-all`'s first catch): the session is now held while the burst is queued.

## 2026-10-17 (later) — a contracted loop left through its own shim (KI-277); `std/registry` (ADR-412)

bedit's pty drivers all timed out: under `nest run`'s armed contracts, `vt/feed` took ~55 ms
per character on a 140×34 screen. The cost scaled with the SCREEN, not the input, and was gone
at tier 0. A tail call to the function itself is `Inst::SelfCall`, a back-edge that never
resolves its head, so ADR-383's same-module rewrite never reaches it. Its hot-reload guard then
asked the PUBLIC name "is this still us?", and armed, that name holds the shim. The loop
always answered "redefined" and left through its own contract. vt's `print-graphemes` carries
the whole terminal, so each character paid a whole-grid `type-matches?`. Since `19a813ab`, a
fresh frame checks at its first back-edge, so every iteration took the same exit. The guard
now reads the name as the module's code does (`contracts::self_call_binding`). The boundary
fixture gains a contracted `countdown` whose last self-call passes a string: armed, it raised
before. All twenty bedit drivers pass armed again.

What is left is the boundary itself: an outside `vt/feed` still pays three whole-terminal
checks, ~150 ms. `type-matches?` interprets the type on every call, resolving `deftype`
aliases each time, ~10 µs a cell. Compiling a contract's types into predicates once, when its
shim is built, is the fix. It is a change to `std/contract` worth its own ADR.

`std/registry` (ADR-412) is the other half of the same bedit review. bedit kept seventeen
load-filled tables as `defonce` + `def`, each open to the lost-update race, the `%isolate`
restore and the image omission the kernel's registries were fixed for. The kernel's
`%registry-update!` handles all three but wants a qualified global a project cannot spell. So
`registry` keeps one global, `id → key → entry`, and writes it through the primitive's
two-level path. `reflect/call-if-bound` is the late call its symbol hooks want. Guards in
`tests/std_registry_test.blsp`; the concurrency case reds when `put!` is a plain `def`.

`editor/transient` short rows. bedit's git menus are 139 command rows, 41 switches and 9
options, each a four-key map the formatter spreads over four or five lines, ~750 lines of
`:kind :command`. A suffix may now be a vector, `["p" 'cmd "label"]` or `[key kind target
label extras?]`, normalised once in `transient-open` (and by `spec-suffixes`, idempotently), so
nothing past the spec sees anything but the map it always saw. Guards in
`tests/transient_test.blsp`: the push menu written short is the same menu (expansion, chords,
an option's prompt from the extras, a short sub-menu, identical rows). With normalisation
removed from `transient-open`, the rendering case reds.

`editor/buffer-registry` gains an opt-in mirror. bedit's collab daemon kept a private copy
of the registry for one difference: a buffer process that dies comes back from the
registry's own text, not from whichever holder re-shares first, since that holder may be the
one that missed the last edits. `{:mirror true}` subscribes the registry to each process it
spawns, folds the pushes through `link-fold` + the new `buffer-client/text-apply-action`,
respawns a died process from the mirror under the same key (announced as the
removed-then-added pair frames already relinks on), and seeds a share that overtakes the
`[:down]` from the mirror too. Off by default, so frames is unchanged. Guards in
`tests/buffer_registry_test.blsp`; with the respawn or the fold sabotaged, both mirror cases
red.

`std/editor/list-motion`: bedit moved a selection through a list in four places — the
minibuffer, the completion popup, the context menu, the vertical completion list — and the
copies drifted (the context menu swallowed C-n). The key vocabulary is now data and the index
one function, wrapping next and previous and safe on an empty list, which bedit's copy
divided by. Guards in `tests/list_motion_test.blsp`.

Three costs bedit's performance pass traced into Brood, each O(file) where O(nearby) would
do. `closer-redundant?` and `bracket-match` scanned for the enclosing opener from offset 0
in interpreted Brood (a `)` typed near the end of a 44k-line file: 562 ms). They now start
at `reflect/scan-form-start`, where nothing is open, which is the same answer: 10 ms, all of
it the native scan table. `point-kill-end` parsed the whole text for the enclosing list;
it now uses `sexp-site`'s narrowed window like every other motion (262 ms → 1 ms). And
the tree-sitter cache kept one tree per language, so the view's windowed parse and the
indenter's whole-file parse evicted each other and every keystroke reparsed in full. The
near-hit search now takes the cached tree whose derived edit is SMALLEST and replaces that
entry. The first version took the most recent match, which let a whole-document parse claim
the window's entry, since a small text is "an edit" of anything; the new unit test caught it
on its first run. ~16 ms → ~2.5 ms a keystroke in a 1,300-line Elixir file.

Left as a proposal: a FRESH string pays a whole-text pass on first use, both the char index
(`CharIndex::build`) and the form-scan table, even for an offset near the start. Every
keystroke makes a fresh buffer string, so RET in a 44k-line Brood file still costs ~10 ms of
the indenter's `scan-form-start`. The real fix is incremental: scan checkpoints that survive
an edit, kept on the ROPE, so a keystroke rescans from the checkpoint before the edit rather
than from 0. That is ADR-sized and is not done here.

`%rope-find` walked the rope a character at a time through a ring buffer, about 200 MB/s,
so isearch typing into a failing search on a 1.5 MB buffer paid ~9 ms a key. It now searches
each leaf with the standard library's substring search, carrying the previous leaf's last
`m - 1` characters in front so a match across two leaves is found. Folding is one char to one
char, so the char index found in a folded leaf is an index into the rope. Guard
`find_tests::chunked_search_agrees_with_a_flat_search`: every start offset over a many-leaf
rope agrees with a flat search, for straddling, self-overlapping, multi-byte and absent
needles, folded and not; with the carry dropped, it reds. 9 → ~3 ms exact, 10 → ~5 ms folded.
`find_last_before` (C-r) still re-scans forward from 0, now at the faster rate. A true
backward leaf search is the next step if C-r ever shows up in a profile.

`ui-memo` nesting (ADR-336). `%ui-harvest` stored each fragment's ops FLATTENED, with the
inner markers already spliced out. When an outer fragment hit, `ui-memo` replayed those
flattened ops, the inner fragments were never re-tabled, and the next turn the outer missed
they all missed too. A pane-body memo around bedit's per-row memos was useless for that
reason, and it is what stands between a memo-hit frame and a millisecond. The entry now
keeps the ops as emitted. The painted frame is unchanged, since the harvest still flattens
on the way out. Guard in `tests/ui_test.blsp` "an outer fragment that hits keeps its inner
fragments in the next table"; with the flattened storage put back, it and the existing
nesting test red.

## 2026-10-18 — the per-process floor: three caches become `SmallMap`s; the ratchet measures a slope

Picked from `runtime-frontier.md` / brood-benchmarks `FRONTIER.md`: residency, not spawn
bookkeeping, is the spawn-and-park gap to the BEAM (spawn machinery was measured at 17% of
it on 2026-07-29), so the lever is bytes per parked process. `-Zprint-type-sizes` puts
`Process` at 1 504 B, 1 400 of it the inline `Heap`: 288 B of slabs and ~80 small fields,
with no single fat one.

- The closure-template, const-closure and ability-dispatch memos were `HashMap`s, which
  spend four buckets on a first entry; they are `SmallMap`s now.
- `SmallMap`'s spill variant is boxed, so each map is a `Vec`'s 24 B, not a `HashMap`'s 32
  (`small_map::tests::the_map_costs_a_vec_inline`).
- The package-rooting memo moved into `ColdHeap`: outside a package `root_qualified_ref`
  answers `None` without memoising, which is what the uncached rule already returned.
- The contract alias-by-suffix memo is boxed at first use (64 B inline before).
- The mailbox's dead-alias set (ADR-244; a 72 B inline array) is boxed only while it
  holds something, and its pending exit reason (a 32 B `Message`) is boxed: both are empty
  on almost every process (`mailbox::tests::dead_aliases_cost_nothing_until_used_and_free_when_emptied`).

**The ratchet was measuring the wrong thing.** `process_floor.rs` divided one fleet's
growth by N, so one-time allocations in the window counted as per-process: 3 587 B/proc at
n = 5 000 against 3 131 at 40 000 on one binary, and ~200 B between two builds of the same
tree at 20 000 — enough that its 4 040 bound had gone stale by ~800 B without anyone
noticing. It now spawns a warm-up fleet, then measures the slope over a second one. The
slope is stable to ±4 B and does not depend on `treesit-grammars` (an apparent 220 B
difference between feature sets was the old probe's noise).

Measured on the slope, test profile, clean build of `8f345ed9` vs the change: **3 195–3 207
→ 2 978–2 979 B per parked process (−222 B, −6.9%)**: the heap half 3 051–3 055, the
mailbox half the rest. Bound lowered 4 040 → 3 040; the heap-half-only tree (3 053) reds it. Instructions (`perf stat -e instructions:u -r 5`, `BROOD_J=1`,
`BROOD_NO_CHECK=1`, release, interleaved twice, outputs checked equal): closure creation
(3 literals × 400k) 3.157G → 3.136G, ability dispatch (300k × 3 ops) 5.326G → 5.304G,
spawn + park of 50k 1.67–1.69G → 1.64–1.67G — flat to slightly better on all three
(measured on the heap half; the mailbox half touches only the retire path and a `None`
check on delivery).

## 2026-10-18 (later) — clippy, a memory test's race (KI-279), and KI-280 filed

The floor commit failed CI's clippy: `Box<HashMap>` trips `clippy::box_collection`, and the
box is the point (it pays only on a spill), so the variant carries an `allow` with that
reason. Run clippy with CI's `--all-targets --all-features` before pushing; a plain run passes
what CI rejects.

CI's tree-walker job had failed on `8f345ed9` (both tries) in `memory_pressure`'s stuck-victim
test: the waiting root died of `E0043` instead of the leak. After the stuck hoarder's 250 ms
grace the leak held ~40-50 MB, under the eighth of the count (~55 MB) a process must hold to be
stopped, so nobody was attributable and the error went to whichever process reached a
safepoint first. The leak is now primed to 64 MB before the hoarder fills up (KI-279).
Passes at every tier on four and twelve cores. On two cores the tree-walker still fails, for a
different reason: the leak's published footprint jumps by about the hoarder's size and the
selector raises in the leak. Filed as KI-280 (open) with the trace; it reproduces on demand.

## 2026-10-18 (later) — KI-280: `os/cmd` blocked its worker without saying so

KI-280 was starvation, not misattribution. With timestamps on the pressure trace, the leak
did not run at all for the hoarder's whole `sleep 20`: on a two-worker pool, worker 0 sat in
the hoarder's `os/cmd` and worker 1 in the root's tree-walked `receive`. The `receive` marks
its worker dirty-blocked (ADR-100 §7.4); `os/cmd` did not, so the woken leak was routed to the
worker inside the command and no overflow drainer started. `os/cmd` now holds `dirty_block()`
for its wait. Guard `blocking_native::a_process_runs_while_every_worker_waits_in_os_cmd`
(`BROOD_J=2`, two blockers, every tier), red without the guard. The stuck-victim memory test
passes at every tier on two, four and twelve cores.

## 2026-10-18 (later) — the rare slabs cost a pointer until used

A heap's slabs are twelve `Vec`s, one per kind, in each of two generations. Five hold kinds most
processes never allocate — bigints, decimals, ratios, ropes and local natives — and are now
`slabs::LazyVec`: one pointer until the first push, dereferencing to an empty slice, and back to
one pointer when `shrink_to_fit` finds it empty. Every slab macro (`alloc_slot!`, the flush
leaves, the region readers) compiled unchanged against it.

Parked process (slope, test profile): **2 978–2 979 → 2 875–2 876 B (−103 B)**; the floor bound
moves 3 040 → 2 940, which the previous tree reds. Guard for the type itself:
`slabs::lazy_vec_tests`. Instructions (`perf stat -r 5`, release, `BROOD_J=1`, interleaved
twice, outputs equal) against `911e6474`: factorial-300 bignums 877-882M → 889M (+0.8-1.4%),
harmonic ratios 2.018G → 2.020G (+0.1%), 200k rope inserts 1.708G → 1.723-1.728G (+0.9-1.2%),
spawn + park 1.67-1.70G → 1.64-1.66G. The rare kinds pay one pointer hop, about a percent on a
loop made of them; every process pays 103 B less.

KI-281 filed as a watch: CI's tree-walker job on `911e6474` failed a JIT fast-link test after
its 90 s deadline, once; 70 local runs (plain, loaded, single core) did not reproduce it. The
test helper's failure now reports every compiled arm's JIT state, so the next one explains
itself.

## 2026-10-18 (later still) — the rarely-written heap fields move off the inline `Heap`

The GC statistics (copied, reclaimed, three pause timings), the `proc/flag` settings (max heap
and its pending hit, send errors, no-break, GC trace), the debugger's trace context and a test's
shared-code floor override now live in `HeapExtras`, boxed at the first write
(`Heap::extras`/`extras_mut`). Every read treats `None` as the defaults, and the two clears that
could allocate — `set_proc_mem_limit(None)` and `set_trace_context(None, _)` — return early when
nothing is set. `gen_cache_retired` is a `LazyVec`, and the checker slot is a
`OnceCell<Box<RefCell<CheckHeap>>>` (one pointer). Guard for the no-allocation property:
`accounting::extras_tests::reading_the_defaults_leaves_the_extras_unallocated`, red with the
no-op-clear early return removed.

Parked process (slope): **2 875–2 876 → 2 747–2 748 B (−129 B)**, against the BEAM's 2 704 (338
words); bound 2 940 → 2 800, which the previous tree reds. Today: 3 195–3 207 → 2 747 (−14%).
Instructions against `c6a0c600` (`perf stat -r 5`, release, `BROOD_J=1`, interleaved twice,
outputs equal): a depth-14 binary tree built and walked 20 times 1.008G → 1.008-1.011G, closure
creation flat, ability dispatch −0.1%, spawn + park 1.67-1.69G → 1.64-1.65G.

## 2026-10-18 (late) — the read that arms CPU accounting times its own quantum (KI-282)

The nightly's GC stress job failed `cpu_time_test`'s self-timing test. Not a GC fault: the first
`:cpu-us` read arms quantum accounting from inside a quantum that began unarmed, so that quantum
never got a clock, and a process timing its own work across the runtime's first read got 0.
`pool::arm_cpu_accounting` starts the running quantum's clock at the arming moment, on executors
only. Guard `cpu_time_first_read`, red without it. Also on the way to a release: the
`jit_eq_join` polymorphic-param tests wait out a queued re-lowering before reading its state,
and two gates upstream's commits left red (two process-wide statics, one bare name) are green.

## 2026-10-18 (evening) — the tree-walker stops reporting contract shims; what `BROOD_TIER=0` does not reach

**Contract errors on the tree-walker.** The handoff's known limit was wider than written:
under `BROOD_TIER=0` every error that left a contract shim reported `std/contract.blsp`, not
only a caught contract error. A thin contracted function's primitive runs at the shim's own
`(%contract-orig a b)` (the tree-walker's thin-wrapper redirect goes straight to `+`), and a
failed argument check raises in `%contract-error`, so a `(+ "s" 1)` the user wrote read
`std/contract.blsp:162` and a bad argument `:30`, with trace frames in the same file. The VM
has filtered shims out since KI-184. The tree-walker now applies the same rule
(`eval.rs` `untag_shim`): a frame running a contract shim records the file its forms carry,
and an error leaving it with a position in that file is re-tagged at the user's call of the
contracted function, its trace entries re-pointed there, and the shim is not named. Non-tail
calls agree with the VM exactly. After a TAIL call into a shim the VM, having reused the
frame, names the call that entered the frame; the tree-walker names the tail call itself.
Guard `crates/lisp/tests/contract_error_position.rs` (every tier; red at `TreeWalk` with the
shim test disabled).

**Why the guard is a Rust test.** Written first as cases in `tests/contract_test.blsp`, it
passed with the fix disabled: a `--test` body runs in a spawned process, and
`run_process_body` runs any VM-eligible process body on the VM whatever `BROOD_TIER` says.
Only top-level forms honour the ceiling (`vm_run_bc`'s carve-out exists so that `brood
file.blsp` does not "silently run on the VM"). So the tree-walker differential —
`make test-both`'s second half and CI's `differential (tree-walker)` job — tree-walks each
test file's top-level forms and runs the test BODIES on the bytecode VM. Recorded as an open
question in the handoff, not changed: tree-walking process bodies makes every `receive` block
a worker.

**Brood column refresh at 099abd59 (brood-benchmarks).** `collatz` +7.1% wall, +6.6%
instructions, and `primes` +3.0% instructions are `ef80547d` (KI-274: a native self-loop
snapshots the epoch at entry and compares it on the back-edge poll), measured on rebuilt
`ef80547d~1`/`ef80547d`. The poll runs once per tick batch, but the cost scales with
iterations (~2.7 instructions each on `collatz`), so the suspect is the entry epoch held live
across the loop body — not yet verified. `base64` −38% is the row switching to the native
codec (`a85044ab`), not language speed.

KI-281: CI's tree-walker job also sets `BROOD_NO_STDIMAGE=1` and `BROOD_NO_PRELUDE_IMAGE=1`,
which none of the earlier local attempts did. With them: 400/400 in parallel (16 at a time),
and with the background compiler delayed 50-500 ms before lowering or before install, GC
stress and one worker, all green; one full tree-walker job, CI's exact shape, 1939/1939. Still
a watch.

## 2026-10-18 (late) — `std/mime`: mail messages; a lossy UTF-8 read; backward rope search walks back

bedit wanted to open an `.eml` and show what a reader means by it, and Brood could not say any
of it: no header unfolding, no RFC 2047 encoded words, no quoted-printable, no multipart, no
legacy charset — and `string/bytes->` raised on the first invalid byte, which mail is full of.
**`std/mime`** (pure Brood over `regex` + `encoding`) reads a message into a part tree
(`parse`), and decodes what a reader sees: `header` / `header-values`, `decode-header`
(adjacent words pool their bytes, so a character split across two still decodes),
`parse-params` (quotes, RFC 2231 continuations and `*=utf-8''…`), `quoted-printable-decode`,
`decode-charset` (UTF-8, US-ASCII, ISO-8859-1/-15, windows-1252), `body-bytes` / `body-text`,
`leaves`, `text-part` (plain over HTML in an alternative), `attachment?`, `filename`. Nothing in
it raises on bad mail: a word that does not decode stays as written, broken base64 reads as
its text. 21 tests (`tests/mime_test.blsp`).

**`(string/bytes-> b :lossy)`** — `%utf8-bytes->string` takes an optional flag and reads each
invalid sequence as U+FFFD (`String::from_utf8_lossy`). The strict default is unchanged.

**`buffer-search-backward` costs the distance, not the offset.** `find_last_before`
(`text_search.rs`) rescanned forward from offset 0 keeping the newest match — 33–41 ms at the
end of a 40 MB mailbox on every "previous message", which bedit had worked around with a
chunked backward search of its own. It now reads 64K-char slices back from `before`, each
overlapping the next by the needle's length, and `rfind`s each. Guard
`backward_search_agrees_with_the_last_forward_match` (overlapping matches, slice-straddling
matches, multi-byte, folded).

## 2026-10-18 (late) — the tab images its std modules; the tab wasm loses a third; Windows checks

b2d's handoff, both items. **The tab** (ADR-414 addendum 2): image paths under `mem:` are bytes
held in memory, so the sectioned stdlib image works where there is no filesystem.
`stdimage/build-at` writes one anywhere; `crates/tab` gained `std_image(modules)`,
`use_std_image` and `loaded_modules`. The image holds only the std modules the game CALLS —
learned by running it on an image of all of std, because a source boot also loads every module
a body merely names. b2d demo, first frame in node: ~540 → ~250 ms; the image is 347 KB
gzipped. Four new default-on features — `brotli`, `checker`, `image-formats`, `toolchain` —
that the tab leaves out and `crates/cli` and the playground ask for: the tab wasm 10.8 → 7.4 MB
(2.61 MB gzipped, was 3.71), with the name section dropped at bindgen.

**Windows.** Cross-checked from Linux (`cargo check --target x86_64-pc-windows-gnu
--all-targets` with build scripts' C stubbed): the library already compiled; only test code
reached for Unix. The dist tests' socket pairs are loopback TCP now (they run everywhere), the
symlink and cookie-mode checks gate on `unix`, two warnings went. Behaviour: the file lock behind
the compare-and-swap writes was a no-op off Unix and is `File::lock` (`LockFileEx`) now; the
GPU window's exit gate registers its `atexit` on Windows too (`libc` is a Windows dependency for
the CRT's). CI: `windows-check` (the workspace, every target, GPU gui and audio) and
`windows-smoke` (build `brood`, run a program that spawns, sends and requires; allowed to fail
until it has passed once). macOS: the same cross-check with every target is clean.

Also: `temp_purge_coverage_test` was red on `main` — `project_test.blsp`'s new `brood-gate-` and
`brood-shadow-` fixtures were never purged; they are.

## 2026-10-19 — ADR-416 stage 1: what `BROOD_TIER=0` does not reach, by audit

`BROOD_TIER0_AUDIT=<file>` (catalogued under Engine selection): under a tier-0 ceiling,
`vm_run_bc` (the one place bytecode starts) appends the call path that started it, once per
distinct site per OS process. The site is read off a rendered backtrace: the frame names are
unqualified debuginfo names, so a frame counts as ours unless its location is in std or a
dependency, and is written `name@path:line`. A trace without `vm_run_bc` in it still writes an
`unplaced` line, so the audit cannot go quietly empty. Guard: `crates/cli/tests/tier0_escape_audit.rs`
(names the process-body escape at tier 0; writes nothing at tiers 1 and 2), each half sabotaged
red.

**The inventory**, over the whole suite at `BROOD_TIER=0` (1943 tests; processes per site):
- `run_process_body`: 181 (from the scheduler's `drive`, `test_drive_quanta`, `overflow_drain`).
  The known escape. 64 more sites are VM code started from inside those VM bodies; they go
  with it.
- **Tree-walked code reaching the VM through a ceiling-blind door**, each independent of the
  frame stack:
  - `compile::apply_value`, from primitives calling back into Brood: `num_multi_dispatch`
    (`+ - * /` on a user number type) and `compare_multi_dispatch` (`< <=`), and `record_seq`
    (`first`/`rest`/`empty?` on a seq-protocol record). `apply_engine` is the ceiling-honouring
    twin; 9 `apply_value` callers in all.
  - the `receive` matcher: `mailbox::apply_matcher` resolves it with `hof_resolve` and runs it
    with `hof_apply_step`, whatever the ceiling.
  - `eval_in` (`builtins/evaluation.rs`) calls `compile::run` per form, not `run_top_form`.
- One Rust unit test calls `vm_run_bc` directly (`vm_run_bc_captures_and_resumes_a_suspend`);
  not an escape.

Also: `MEMORY_IMAGES` (e9ddd4c6) was missing from `process_wide_statics`' list, red on both
engines in CI; listed.

## 2026-10-19 (later) — the three ceiling-blind doors closed; tier 0 now escapes only through process bodies

The audit's three doors from tree-walked code into the VM each take the ceiling now:
primitives calling back into Brood (`num_multi_dispatch`, `compare_multi_dispatch`,
`record_seq`) go through `apply_engine` instead of `apply_value`; `mailbox::apply_matcher`
resolves no arm below the bytecode tier (as `Reducer` already did), so the matcher tree-walks;
`%eval-in` runs its forms with `run_top_form`. Guard: `tier0_escape_audit.rs`'s
`tree_walked_callbacks_stay_on_the_tree_walker`, a top-level tier-0 program through each door,
audit empty; each of the five sites sabotaged red. The matcher door needed a specific shape to
be reachable at all: a guard (a plain pattern is a native spec, ADR-409) inside a `defn` (an
inline top-level closure has a LOCAL body, so `compiled_arm_for` has no key for it).

`make test-both` green (1946/1946 on each engine) with the audit armed: every remaining site is
`run_process_body` or VM code started inside such a body, plus the one Rust unit test that
calls `vm_run_bc` directly. That is ADR-416 stages 2–5's whole job.

## 2026-10-19 — an editor's features leave std (ADR-417)

`std/editor` had collected bedit's features alongside the toolkit: Org-mode, Magit's transient
menus and section trees, form buffers, a highlighter for each file format, and a mail parser.
Each had one user, bedit, and each was a std name that versioned with the kernel. ADR-417 sets
the test: a std module is a mechanism, something besides one application uses it, and it has
to version with the kernel. Ten modules failed it and moved to bedit with their tests:
`org`, `transient-core`, `section`, `formbuf`, the `lex-*` highlighters and `mail-message`
(previously `mime`). Two shared test files were split, so the parts that test code still in
std stayed here: `lexer_test.blsp` keeps the `editor/lexer` cases from `configs_test`, and
`layers_test.blsp` gains the `shebang-interpreter` cases from `shell_test`. The KI-187 test in
`deftype_through_imports.rs` used to borrow `editor/section`'s `row` to collide with `vt`'s;
it now declares its own.

## 2026-10-19 (later still) — ADR-416 stage 2a: the tree-walker's pending work is a heap frame stack

`eval/machine.rs`. `eval_tail_loop` is now a driver over an explicit stack of continuations
(`Kont`): an `if` test, a cons body's non-last forms (`do`/`let`/`letrec`), a closure body's
non-last forms, a `let`/`letrec` binding's value, `def`'s value at tier 0, a vector/set/map
literal's elements, a computed head and a call's arguments each push one and run on the same
loop, where each was a nested Rust `eval`. A continuation is exactly the old nested call: it
sets the pushing frame's trace tracker and lazy-expansion count aside, and an error popping it
closes the inner frame's trace (`close_frame`, shared with `eval` and `apply_closure`), takes
the sub-form's position where the old site used `eval_at`, and drops its operand roots as
`root_scope` did. It holds `Root`/`EnvRoot` slots only, so both collectors relocate it in
place. Leaves (atoms, bound symbols) are read in place, so `(+ a b)` pushes nothing.
`eval_arguments`, `tail_of_cons` and `bind_sequential` are gone into it.

Runaway non-tail recursion now stops at a frame count (262144,
`recursion too deep: exceeded the tree-walker's …`, `E0044` as before); the native-stack byte
budget still guards a run re-entered through native code. `docs/error-codes.md` and
`brood-debug-skill.md` say so. Still native recursion (stage 2b): macro expansion, `&optional`
defaults, the passthrough redirect's slow head lookup, a seq-view realised for `apply`, and
natives calling back (`apply_closure` runs a machine of its own).

Evidence: `make test-both` 1946/1946 on each engine; `gcstress-all` at `BROOD_TIER=0`, all 297
files clean; all thirteen fuzz generators (tree-walker as reference), 3000 programs, no divergence, crash or
stale program. Guard:
`crates/cli/tests/tree_walker_frames.rs` (a 200 000-deep non-tail recursion completes at tier
0, a runaway one stops at the frame limit, catchably); red with HEAD's `eval.rs` put back.
Not measured: tier-0 speed (`make ab` with `BROOD_TIER=0` belongs on the benchmark machine).

## 2026-10-19 (evening) — ADR-416 stage 2b: entering a closure is one path, and its defaults are frames

Inside one tree-walked run nothing recurses natively now. `Machine::enter` is the single way
into a closure body, for three callers: a call the running frame makes; a macro expander, run
as a frame under a new `Expand` continuation whose value replaces the call (it was a nested
`apply_closure`); and `apply_closure` itself, which runs the whole body on one machine instead
of a nested `eval` per non-last form. `bind_params` now runs nothing: it binds what the call
supplied and hands back the `&optional` defaults still owed, which `enter` evaluates in the
closure's scope as frames (`KontKind::Defaults`). An error leaving one is attributed as a
failed bind, as before (the call's position, a name-only trace entry for the callee). The
passthrough redirect's slow head is a symbol lookup (`lookup`, shared with `step`), not an
`eval`. `eval_at` is gone.

A native-boundary entry keeps two pieces of bookkeeping its old loop had. Its trace entry
starts as the callee's name with no call site (`TwFrame::boundary`). An error leaving its
body's tail takes the tail form's position (`TailTag`, a frame register beside the trace
tracker): `(try (do … (let)))` reports the `do`'s line at tier 0, as the VM does; without the
tag it reports no position. `apply_closure` keeps the native byte-budget guard: a native
calling back into Brood is still real Rust stack, one machine per level.

Observable: a recursion through an `&optional` default
(`(defn depth (n &optional (acc (… (depth (- n 1))))) acc)`) now reaches 100 000 levels at
tier 0; before, each level was a nested native `eval` and the run failed. Guards in
`crates/cli/tests/tree_walker_frames.rs`: the defaults depth (red with HEAD's `eval.rs` and
`machine.rs` put back), and the boundary tail position (red with `TailTag` made a no-op).

Evidence: `make test-both` 1949/1949 on each engine; `gcstress-all` at `BROOD_TIER=0`, all 289
files clean; all thirteen fuzz generators (tree-walker as reference), 2990 programs plus
metamorphic variants, no divergence, crash or stale program. Not measured: tier-0 speed.

## 2026-10-19 (night) — ADR-416 stage 3: `try` and `binding` are tree-walker frames

Under a tier-0 ceiling a call of `%try`, `%try-described` or `%binding` whose thunk is a
closure no longer runs the native: the machine pushes a `Try` or `Binding` continuation and
enters the thunk across a native boundary (`Machine::enter`, as the native's `apply_engine`
reached `apply_closure` at tier 0, so traces and positions are unchanged). A value pops the
frame as the call's value. An error that is not a control signal unwinding into a `Try` frame
runs the handler (`Machine::catch`) on what the native gave it (`builtins::handler_args`, now
shared), as a frame whose value is the call's (`Callback`); a control signal passes through,
as `run_guarded` re-raised it. A `Binding` frame truncates the dynamic stack on the way out,
by a value or an error; the bindings are pushed by `builtins::binding_enter`, shared with the
native, whose errors keep the call's position. A non-closure thunk, and every tier above 0,
still runs the native (there `apply_engine` routes the thunk to the VM).

Found on the way, not fixed: under N active `binding`s, every global read on the tree-walker
scans all N dynamic entries before the global table (`Heap::env_get`). Only dynamic names are
ever on that stack, but `is_dynamic` takes the shared `RwLock`, so asking it first is not
obviously cheaper. Harmless at ordinary depths; a 50 000-deep recursion through `binding`
takes 5 s at tier 0 (0.06 s on the VM, which reads globals through its caches).

Guard: `tree_walker_frames.rs` runs a 20 000-deep recursion through `try` and `binding` at
tier 0, checking the handler's value and the dynamic stack's restore; red with the
interception disabled. Evidence: `make test-both` 1950/1950 on each engine; `gcstress-all` at
`BROOD_TIER=0`, all 289 files clean; all thirteen fuzz generators, no divergence, crash or
stale program.

## 2026-10-19 (late night) — ADR-416 stages 4 and 5: a tier-0 process body runs on the tree-walker, and its `receive` parks

At `BROOD_TIER=0` a process body and each top-level form now run on the tree-walker's
**capturing machine** (`machine::run_captured`). A `%receive` that finds nothing, and a
reduction budget that runs out at a step's safepoint, leave the machine's frames intact and
return them to `run_one` as `Continuation::Tw(TwSuspended)` beside the VM's
`Continuation::Vm`; the scheduler parks, re-enqueues or resumes it on any worker
(`resume_captured`). A `receive` is redone from scratch on resume, as the VM re-runs its
rewound call; a preemption redoes the step it stopped before. A `receive` under a native
that calls back into Brood still blocks its worker, as on the VM. `run_process_body`'s
"VM-eligible body runs on the VM whatever the ceiling" branch and `run_program_body`'s
synchronous tier-0 branch are gone, so under tier 0 no program starts bytecode: the escape
audit over the whole workspace suite at tier 0 records one site, a Rust unit test that calls
`vm_run_bc` itself.

The audit stays a diagnostic, not a runtime hard check: nothing on a program's path reaches
the VM any more, and the paths that do (unit tests driving `vm_run_bc`) are legitimate. It
is kept honest by a positive control in the lisp crate
(`the_tier0_audit_records_bytecode_started_under_a_tree_walk_ceiling`, red with the hook
disabled), since the CLI guard now asserts an empty audit, which a broken audit also writes.

Test bodies really tree-walk at tier 0 for the first time, and that found four things:

- **A real `let` divergence, older than this work.** A `fn` that calls itself by its `let`
  name was unbound inside itself at tier 0 whenever a list-valued binding came before it:
  `(let (x (+ 1 2) f (fn (n) (if (= n 0) x (f (- n 1))))) (f 3))`. The 2026-10-08 rule
  (a binding after a list-valued RHS goes into a fresh child frame) opened that frame after
  the closure had captured its parent. It now opens before a `(fn …)` RHS is evaluated
  (`BindState::open_frame_for_fn`), which is what the VM's `compile_let` already matched
  (`letrec_self`). Guarded in `tests/let_capture_shadow_test.blsp` (in-process and across a
  `spawn`) and the differential corpus; all three red with the fix removed.
- **The frame cap stopped tier 0 short of the VM.** An inline `try` recursion costs two
  continuations a level, so `MAX_TW_FRAMES = 1 << 18` died near 131 000 levels where
  `vm_nested_stack_guard_test.blsp` expects 200 000. Now `1 << 19`, not the VM's `1 << 20`:
  a tree-walked level holds several KB of heap besides its frames, and at `1 << 20` a runaway
  `%try` recursion hit the runner's 2 GB memory limit before the catchable depth error. A plain
  runaway at tier 0 now peaks near 1 GB. `KontKind`'s three per-form states are boxed
  (`Bind`, `Literal`, `Defaults`), which takes a continuation from 592 to 360 bytes. That
  file's two native-`%try` cases ask for two million levels, past every engine's limit; at tier 0
  `%try` is a frame since stage 3, so 200 000 no longer reached the bound they assert.
- **Line coverage is a bytecode instruction**, so `nest test --cover-lines` at tier 0
  measured 0%. `nest` now pins the VM ceiling for `--cover-lines`/`--cover-branches` whatever
  `BROOD_TIER` says (it used to keep an explicit tier 0).
- **`quantum_tail_panic.rs`'s spinners** were sized for the VM: four 40M-iteration loops on one
  worker outlast its 30 s `after` on the tree-walker. 4M is still thousands of quanta on
  every engine.

Guards: `tree_walker_frames.rs` `tier0_receive_parks_and_resumes` (three processes chained
through `receive`; it asserts the scheduler's `BROOD_SCHED_DBG` log shows tree-walker
continuations parked at `receive` and resumed. The answer alone cannot: a sabotage that made
the `receive` block still completed the chain, because the runtime makes up for a blocked
worker. Red with that sabotage); `tier0_escape_audit.rs`
`a_tier0_process_body_stays_on_the_tree_walker`. The in-language suite's wrapper now takes
370 s at tier 0 (196 s before, when its bodies ran on the VM), so its nextest budget is 1800 s.

Evidence: `make test-both` 1952/1952 on each engine; `gcstress-all` at `BROOD_TIER=0`, all 289
files clean; all thirteen fuzz generators, no divergence, crash or stale program.

## 2026-10-20 — the slow gates are green; `binding` stops costing every global read, and a tier-0 level costs less

**The slow gates on the ADR-416 tree** (the user's item 1): `make gcstress` clean at the
default ceiling and at `BROOD_TIER=0` (12/12 each); the distribution tests 41/41 in five
loops on each engine; `make runtime-soak`, 495 rounds clean in 600 s; KI-281's test 300/300
at sixteen in parallel in CI's tree-walker environment, so it stays a watch. CI on `9fda7da8`
was red on `nest check` — an unused binding in a test of mine. The pre-push hook ran CI's
checker gates only for a change under `std/` or the checker, so a push of test files alone
skipped them; it now checks the changed `tests/`/`examples/` files (red on that file).

**A global read under N `binding`s scanned all N.** `Heap::env_get` searched the dynamic
stack for every global name, dynamic or not, so a deep recursion through `binding` was
quadratic on the tree-walker: 5.5 s for 50 000 levels in a release build. The stack now
keeps a count of each symbol on it (`core::heap::Dynamics`), and a read scans only for a name
that is on it; the scan finds the innermost binding first. Now 0.22 s in a debug build. The
stack and its index are one box, allocated by the first `binding`: a first cut put the index
beside the `Vec` and `process_floor` caught the 8 bytes on every process (2803 B against its
2800 B floor). One word in place of the `Vec`'s three, a parked process now counts 2778 B.
Guards: `dynamics_index_tests` (the index is a count, not a set — red when a pop clears it),
and `tree_walker_frames.rs` `tier0_global_reads_under_deep_binding_stay_linear`, 100 000
levels timed against the same recursion without `binding` (25.4 s vs 0.24 s with the scan
back, 0.3 s with the index).

**A tree-walked level of recursion cost ~1 KB, the VM's ~0.2 KB.** Measured at 400 000
levels: three quarters of the growth was the frame stack, a `Vec` that doubles and briefly
holds the old buffer and the new one. It is now a `KontStack` of fixed 1024-frame segments
— a shallow machine grows exactly as before — and `TwFrame`'s contract-shim fields, set only
inside a shim, are boxed (a continuation is 320 bytes, from 360; 592 before yesterday's
boxing). 400 000 levels peak at 338 MB (480 MB), and a runaway recursion to the frame cap at
~750 MB (989 MB). What is left is intrinsic to tree-walking: an environment frame per call
(~280 B a level at the cap) and the root stack, which the JIT reads in place and so stays one
buffer. Guard: `kont_stack_tests` (LIFO across segments, segments set aside — red with
segmenting disabled).

Evidence: `make test-both` 1955/1955 on each engine; `gcstress-all` clean at the default
ceiling and at `BROOD_TIER=0` (289 files each); all thirteen fuzz generators, no divergence,
crash or stale program.

## 2026-10-20 (later) — what ADR-416 costs at tier 0, measured

**The owed tier-0 A/B**, as this machine can answer it: instruction counts
(`perf stat -e instructions:u -r 3`), which load does not move, not times. Base `a3b7b99f`
(the last commit before stage 2a) against HEAD, both through `make release-brood` with the
same `config.mk`, both on a `:live` stdlib image, the base measured twice as the control
(every row's control within 0.05%). At `BROOD_TIER=0`, on `ab-bench`'s default rows:

| row | Δ instructions | | row | Δ instructions |
|---|---|---|---|---|
| fib | +9.0% | | loop | +5.2% |
| bintree | +10.0% | | collatz | +6.7% |
| nqueens | +6.3% | | primes | +6.0% |
| nbody | +8.8% | | json | +5.9% |
| sieve | +3.0% | | sort | +8.8% |
| pfib | +1009% | | | |

The single-process rows pay 3–10% for pending work on heap frames instead of the native stack.
`pfib` is the point of ADR-416, not a regression: on the base its process bodies ran on the VM
whatever the ceiling said, and now they tree-walk — 11× is the engines' ratio. At the default
ceiling the same eleven rows are flat, every one within ±0.6% (controls to 0.46%), so the
tree-walker's cost does not reach the VM path. A wall-clock `make ab` on the benchmark machine
would only restate these as times.

**Two reds on main** (`bytes_literal_compiles`, and `cargo-deny` on RUSTSEC-2026-0378/0379)
were diagnosed here in parallel with their fixes, which landed as `00a42c06` and `18d66cfb`.
One finding worth keeping: the test's dirty receives were not its literal's. Its fifty processes
were the first to call `bytes/length`, and under a live stdlib image `bytes` loads on that first
use, every process but the loader waiting for it inside the global-miss path (`global_miss` →
`ensure_required` → `require-one`), a native frame where a `receive` blocks by design — 0 in 15
with `BROOD_NO_STDIMAGE=1`, 0 in 15 with the literal kept and `bytes/length` dropped.


## 2026-10-21 — a wait for another process's module load parks at tier 0 and at tier-up too

`bbd6194f` landed the VM half while this was in progress in parallel: a global miss on a
module another live process is mid-load of goes to the prelude's `%lazy-global`/`%lazy-call`
as an ordinary frame, so `%require-await`'s `receive` parks instead of blocking the worker
(ADR-100 §7.4). A broader version here — routing EVERY miss that loads, with the edge flag,
the ADR-366 stale mark and the unbound error's position settled by a finishing builtin — was
dropped in its favour; what it covered beyond the VM was carried onto `bbd6194f`'s mechanism:

- **Tier 0.** The VM's hand-off is in `exec_chunk`, so at `BROOD_TIER=0` the waiters still
  blocked. The machine now makes the same hand-off (`machine::lazy_handoff`, same predicate,
  same `capture` gate): a symbol miss applies `%lazy-global` as this frame's tail, a call-head
  miss computes the head with it as a frame of its own (`KontKind::Head`, which now expands a
  macro the answer turns out to be, as the direct lookup does). A thin wrapper whose inner head
  is mid-load is not redirected, so its own body makes the miss and the hand-off — the
  redirect looked the head up natively.
- **The JIT's tier-up.** `preload_arm_globals` loads what a hot arm names before it goes
  native (ADR-335: native code cannot load, and a native read of an unbound name raises), on
  the VM, through `global_miss` — so it waited for an in-flight load with its worker blocked.
  It now reports whether the arm is ready; with a module mid-load elsewhere it is not, and the
  election backs the hotness counter off (not BAILED) and runs the VM, whose miss parks. A
  first cut that just skipped the module would have let the arm go native with the name
  unbound — caught reading `brood_rt_global` while writing it up.

Measured by the scheduler log (`park pid=N` per park, nothing for a blocked worker) on a module
held mid-load while six processes miss on it, three on a call head and three on a value. On
`3a4de80b` (which carries `bbd6194f`): 6 of 6 parked at tiers 1 and 2, 0 of 6 at tier 0, and the
tier-up program answered `:blocked` after its 20 s wait. With this: 6 of 6 at every tier, also
under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`, and the hot loop answers while the load is held. Guards, `crates/cli/tests/autoload_parks.rs`, each
sabotaged red alone: the tier-0 case (red with the machine's hand-off off, and again with only
the thin-wrapper redirect restored) and a tier-up case — `g` names `slowmod/f` in a branch it
never takes and gets hot while `slowmod` is held; its answer must arrive before the load is let
go (red, `:blocked` after 20 s, with the pre-load waiting again).

**Cost, in instructions** (`perf stat -e instructions:u`, both arms through `make release-brood`
on a live stdlib image, against `3a4de80b`): at the default ceiling the eleven usual rows plus
`startup`, `strings` and `regex` sit inside their base-vs-base band (`bintree` and `regex`
re-run at `-r 10` to settle a noisy first look). At tier 0 the first build was +0.3–0.6% on
every tree-walking row — and the same with both arms built in one worktree, so code, not
placement — although nothing on the hit path had changed: `lazy_handoff` was being inlined
into the machine's `apply`/`step`. Marked `#[cold] #[inline(never)]` (and the two miss branches
moved out of `step` as `symbol_miss`/`head_handoff`), the tier-0 rows are flat: −0.30% to
−0.02%, `bintree` +0.42% inside its own ±0.44% run spread.

**A flake found on the way:** `accept_starvation` (KI-227's guard) failed once in a full suite
with `accepted 31`. Its server timed its first 3 s quiet window from its own start, so a client
slower than that to boot (a loaded machine) left the first window empty: nothing was closed,
and the second accepted 31 and starved with no descriptor ever freed. Reproduced on demand by
starting the client 4 s late, on this tree and on an older binary (`accepted 32`), so not this
change. The server now waits for the client's `connected.txt` before its windows start: green
with the client 4 s late, still red (`accepted 31`) with the reactor's starved-listener retry
forced off.

**And one CI red after pushing it:** the breakage suite's `chaos2_process_links` P16 got
`:noproc` instead of `:badness`. It spawned a child that exits at once and `link`ed it
separately; a child already gone makes the link a `:noproc` exit — the race P15 had and lost to
`spawn-link` on 2026-10-13. Not this change: 1 run in 40 of the whole file under CPU load on
the build before it, 0 in 40 after (the race is that rare locally). P16 uses `spawn-link` now;
0 in 40 under the same load, and the suite passes. P17 and P22 link separately too, but accept
any exit message, so the race cannot make them false.

## 2026-10-21 (later) — a waiter that loses the claim race parks too

The morning's hand-off decided by asking first: a miss checked whether another live process
held the module's load (`module_loading_elsewhere`) and, if none did, loaded it itself through
`global_miss`. Another process can take the claim between that check and `require-one`'s own
CAS, and the loser then met the claim inside `require-one` — under the native lookup, where
`%require-await`'s `receive` cannot park and blocks the worker. Narrow, but it is exactly the
shape a burst of first callers produces: all of them check before any of them has claimed.

The check is no longer the decision. `derive::parkable_miss` arms a one-shot flag for the load
it runs; `require-one` takes it first thing (`%load-may-hand-off?`, beside the edge flag, so no
load nested inside a won claim inherits it) and, if it then finds the claim held or loses the
CAS, answers `:%hand-off` instead of waiting. `ensure_required` turns that into "handed off",
and the miss reports `Elsewhere` exactly as if the up-front check had seen the claim, so the
caller makes the same `%lazy-global`/`%lazy-call` hand-off it already made. A load the miss
wins runs where it did. Every caller that could hand off goes through it: the VM's value read
and call head (`exec_chunk`), the tree-walker's symbol and head misses and its thin-wrapper
redirect (`machine::machine_miss`, which also roots the form and env across the load), and the
JIT's tier-up pre-load, which declines rather than waits either way.

Guard: `crates/cli/tests/autoload_parks.rs`, six cases (call head and value read, tiers 0/1/2).
The race is held open by a new fault flag, `BROOD_FAULT_CLAIM_RACE=<module>`: the first
parkable miss into that module sleeps 500 ms between the check and the claim while the test's
loader takes the claim. With `take_load_hand_off` forced false all six go red ("the racer
lost the claim race and waited with its worker blocked"); restored, the file is green five
runs in a row and under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`.

**What still blocks, and why it is not this fix's to close.** A load inferred at COMPILE time
(a qualified macro head expanding) and a miss inside a nested, non-capturing run both wait
under a native frame. Neither is load-specific: they are the general dirty-block carve-out
(ADR-100 §7.4) that every `receive` under a native frame takes — inside `%try`, a `map`
callback, `reflect/eval`, a module body. The worker is marked dirty, its queue is re-routed,
and an overflow drainer starts if it was the last live executor, so the program is correct and
its other processes keep running; what it costs is one worker for the wait. Closing them means
making those native frames capturable — the direction ADR-416 started at tier 0 — which is a
design project, not a fix.

## 2026-10-21 (later still) — a table dies with its creator, and a dropped one gives its slot back

Decided with the user: ETS ownership (ADR-418). `table/new` records the calling process as the
owner, and `deregister` drops what the dying process still owns before its downs fire. A table
that reaches a global is pinned instead: every write to the shared table of globals now goes
through `promote_global`, and promotion pins any table it meets — a module's
`(def- cache (table/new))` would otherwise have belonged to whichever process happened to trigger
the module's load. `spawn`'s promotion and `to_message` only mark the handle `escaped`.

Reading the store for this turned up KI-283: the linear-map rewrite runs its loop on a private
table and never dropped it, and a dropped table kept its registry slot and 512 KB chunk reservations
for good anyway. A plain tally called ~25 000 times exhausted the 16 GB `ulimit -v`. Freeing
memory therefore meant reusing slots, which needs two things the append-only registry did not
have: a generation in the handle (a stale handle must error, not reach the next table) and a rule
for when nobody else can hold the slot — the handle never escaped, and no JIT'd loop of the dropping
process has the slot base hoisted (`jit_shared`), or the drop is the owner's exit. The rewrite
frees its table with `%table-free`, which may skip the JIT condition because the loop has returned.
A drop now `madvise`s the dense pages away in every case; the mapping stays, so a racing op on
an escaped handle still touches valid memory.

Guards: three `VmSize`-metered reuse cases in `table_address_space.rs` (2000 tables each; ~1 GB of
growth without reuse) and four `table_test.blsp` cases (owner death, `def` pin, closure pin,
stale handle). Each was sabotaged: wrapper free removed, owner release removed, recycling
disabled, generation check removed, pinning disabled — every one went red, and green again
restored.
## 2026-10-21 — a loader's waits park too

v0.42.2 made a process that finds another mid-load of a module park while it waits. The
LOADER still could not: a module whose header `(:use http)` while another process held
`http`'s load waited for it under native frames. 40 such loaders blocked 139 OS workers a run
(stdlib image live); the installed 0.42.2 reads the same. Three frames, each found by a
backtrace at the `dirty-receive-block` site, each removed:

- **The journal.** `require-one` ran every load as `(%with-load-journal (fn () …))`, a native
  that called the thunk through `apply_engine` — under the WHOLE load, so no wait in any load
  could park. It is now a prelude `try` around two natives, `%load-journal-open` and
  `%load-journal-close published?`; `try` is a VM handler frame (`TryEnter`), not a callback.
- **The source load.** `%load-module-source` / `reflect/load` compile and evaluate natively,
  and the `defmodule` expansion's `(require-one …)` per clause runs inside them.
  `%require-header-deps` reads the source's first form (`%read-first`), roots each
  `:use`/`:use-internals`/`:alias`/`:load` target as the expansion would, and requires them
  first, from Brood. Every header clause requires its module unconditionally, so this moves
  loads earlier without adding any. Best-effort: a dependency that fails is left for the
  header, which reports it in context. The image branch's edge replay was a `fold`, whose span
  walks a vector natively; it is a tail loop (`%require-each`).
- **The first-use miss.** The VM rerouted a miss through `%lazy-global` / `%lazy-call` only
  when another live process held the module's claim. A process that found it unclaimed went
  native (`global_miss` → `ensure_required`) and either won — its own load then under that
  frame — or lost the claim race a moment later and waited under it. The route is now every
  module not yet provided and not known absent (`module_to_load_off_native`). `%lazy-require`
  keeps `ensure_required`'s rules: no require-edge for an inferred load (`require-one` now
  passes `%load-edge-skipped?` down as an argument to `%require-one`, so `%lazy-require` can say
  it directly instead of through a thread-local a preempted process could lose), and an absent
  module or a `still loading` cycle falls through to the lookup's own unbound error.

**Sabotage**, each against `a_loader_waiting_on_its_headers_dependency_parks` (new, beside the
waiters' test in `crates/cli/tests/lazy_load_wait_parks.rs`), 0 dirty with all three: header
prefetch off → 118-123; the old narrow route → 4-8; a native `fold` put back under the journal
thunk → 133.

**What is left.** Loading std from source (`BROOD_NO_STDIMAGE`, or no image for the binary)
still waits inside the native compile: the `defmodule` expansion itself calls `string/join`,
and forty first expansions race to load `string` under `macroexpand` — 121 blocks become ~70.
A Brood-side change cannot reach a wait the compiler makes; the image path, which `nest`
builds when missing, is the default.

**Two fixes from proving it green.** The VM's lazy route marked an
arm stale without the `BROOD_TRACE_COMPILE` line the native path prints, which
`run_check_cache` and `stale_loop_handoff` key on; both paths now call
`derive::mark_arm_stale_for_miss`. And `%lazy-require` swallowed any error containing "cannot
find module" or "still loading" — a dependency's error deep in the load included — so a broken
load could surface as an unbound name; it now passes over only the target module's own absence
or cycle.


## 2026-10-21 — a worker busy under a native frame leaves the pool (KI-284)

Proving the loader change above green turned up an older bug. `make test` failed the in-language
suite with 344 assertions — the same 344 twice — nearly all `unbound symbol` in modules loaded
late (`multimap`, `mcp`, `nest`, `markdown`…). Not a loading bug: one test,
`fuzzy/nearest › answers the same in many processes at once`, had hit its 600 s timeout, and the
timed-out unit's clean-up rolled back the modules its processes had loaded while `*features*`
kept them. Pinned to two cores the suite stalled outright, at v0.42.2 as on this tree. gdb showed
both workers running Brood under a builtin's frame (`reflect/eval-string`, `eval`): such a run
cannot yield, and its worker stayed counted as live, so nothing replaced it. One looping eval per
worker and nothing else runs again — not the process meant to kill them, not the runner's timer.

The fix is the CPU twin of the dirty-blocked `receive` (ADR-100 §7.4): a nested run whose quantum
rolls over with work waiting takes its worker out of the pool (`scheduler::cpu_dirty_enter`),
re-routing its queue and starting an overflow drainer if it was the last live executor, and
rejoins when the innermost frames can capture again. `dirty_block`'s accounting is now shared
(`leave_executors` / `rejoin_executors`). Guard:
`blocking_native::a_process_runs_while_every_worker_loops_under_a_native_frame`, red after 20 s
with the transition removed.

## 2026-10-21 — the header prefetch backed out (KI-285)

CI's "suite with contracts armed" job went red on `e6b4f47c` with one failure it could not name:
`scripts/test-contracts.sh` printed the first twenty `error:` lines, and twenty of the suite's
deliberate `process N died: error: …` lines filled them. With the script printing each failure
record whole (`c3a3654a`), the next run said `tests/stdimage_test.blsp:89` — *a root global is
attributed to the module that defines it, not to a dependent*: under contracts the probe's second
`%isolate-discard-loads` window never loaded `string`, so `editor/lineedit` read as introducing
more globals (505) than `repl` (495). Bisected to the header prefetch from `80270034`
(`%require-header-deps`): disabled, the file passed 13/13; enabled, 3/3 red. The mechanism is not
found — whether `string` loads inside the first window depends on whether the pre-flight check, or
its cached verdict, loaded it already — so the prefetch is backed out rather than CI left red, and
KI-285 is open with what was learned. The journal pair, the image branch's tail loop and the VM's
lazy route stay; `lazy_load_wait_parks`' loader case is `#[ignore]`d against KI-285.

A trap on the way, worth knowing: after the bisect the prelude was restored but not rebuilt, and a
dozen runs tested a binary still carrying the change. `nest check` and `brood --check` refuse a
stale binary; a test run only warns on stderr, which the filtered output these runs were read
through dropped. Rebuild `brood` and `nest` after every `.blsp` edit before believing a run.

## 2026-10-21 (later) — what broke the contracts job: a tier-up loading modules into a module's load (KI-285 closed)

The prefetch was the trigger, not the cause. `preload_arm_globals` loads what a hot arm names at
its tiering election (ADR-335 item 4), and the election fires on a call count, so inside a module
load it started an unrelated load at an arbitrary point — journalled as that module's. The
prelude's `get` (its string branch names `string/char-at`) got hot under contracts during
`project`'s load and its tier-up loaded `string` there; the prefetch only moved the counts.
Narrowed by tracing each `require-one` and native miss across the probe's windows, then a
temporary throw at the `string/char-at` miss, which vanished into the pre-load's
best-effort discard. Only `BROOD_TIER=2` reproduced it.

Fix: the pre-load declines while its process is inside a load and re-elects after. The prefetch
from `80270034` is restored and `lazy_load_wait_parks`' loader case un-ignored. Prefetch plus fix:
the contracts-armed suite 6847/6847 three times, `stdimage_test.blsp` 13/13 eight times under
contracts; the decline removed, the probe is red 5/5. Guard: `tier_up_loads.rs`.

One detour recorded in the KI: a worktree build "proved" one side innocent because it had no
stdlib image — and without one this cannot fail.

## 2026-10-21 (evening) — `tier_up_loads` red in the tree-walker job: the test depended on the boot path

`9d75473d`'s CI failed both `tier_up_loads.rs` cases, at tier 1 and tier 2 alike, in the
`differential (tree-walker)` job only — the job that sets `BROOD_NO_STDIMAGE=1`, which the
test's child inherited. With no stdlib image a qualified call HEAD loads its module at
expansion (`derive.rs`: only a head the image vouches is a function waits for its call), so
`hotmod`'s cold `(zlib/gzip "a")` loaded `zlib` before anything tiered up. The KI-285 fix was
not at fault. The cold branch now names `zlib/gzip` as an operand, lazy on both boot paths and
still an `Inst::Global` the tier-up pre-load reads, so the case asks the same question in every
job.

## 2026-10-22 — `nest check`'s peak memory was persistent-map garbage in the lints (large-project scaling item 4)

Re-measured item 4 on a 302 × 3k rig with RSS sampled against the `[check]` phases: nothing
accumulates — the walk sits 0.1 GB over the 0.6 GB load floor — and the peak was two short
spikes in the driver. The queue's "per-chunk driver accumulation" does not exist. Lowering
`BROOD_GC_GROWTH` hides the spikes but `make ab` shows it moving peak RSS both ways on the rows
(`base64` +10%), and the user chose not to set the allocator's purge delay for `nest check`, so
the fix went to the sources. The `[check]` trace now prints the allocator's live and peak bytes
per phase, which is what found them: the lints merged per-file count maps with `%map-assoc`
(+277 MB) and built `name -> paths` for every definition (+100 MB) — both now tallies the
linear-map rewrite builds in place. The module index (v3) carries each file's
qualified-reference prefixes, so `project-preload!` no longer re-parses every file. Sequential
peak 1.15 → 0.81 GB and 50.7 → 45 s; parallel 1.75 → 1.5 GB and 15 → 12 s; identical warnings on
a fixture with duplicate, same-file-duplicate and unused-private definitions against the
previous binary.

## 2026-10-22 (later) — an edit reloads the edited modules and their dependents (ADR-419, scaling item 5)

The project image was one snapshot, so any edit loaded every module from source. Now the
loader keeps a record beside the image (`.brood/image.modules`: the fingerprint, the key's
global part, per-file stamps, each module's registrations, the pinned modules) and, when only
module files changed, reloads the edited modules and everything that may have read them,
materialising the rest. Dependents are read wide — every qualified token in a file's text,
strings included (`:mentions`, module index v4), plus header requires and recorded edges — and
a module whose load evaluated a string is pinned (`%dynamic-loaders`). The edited modules'
registrations are taken out of the restored registries first; the first fixture run caught an
impl record destructured as three fields when it carries five. On the 302 × 3k rig one edit
went 13.2 → 2.7 s. `crates/nest/tests/partial_image.rs` asserts which modules reloaded and
compares every answer with a from-scratch build; dropping the mentions serves a stale value
and fails it.

2026-10-22 (later still): CI found what the hand-run fixture could not — its hooks module had no
globals, so it loaded from source every run. With a section of its own, an unchanged module's
registry rows were lost on the run AFTER a partial rebuild: a registry enters `(%registry-names)`
only when this process writes it, and nothing writes `registry/*registries*` when its filler is
materialised, so the image writer left it out of the root section. The record now keeps the root
section's registry names and the partial rebuild re-declares them (the prelude's
`*require-edges*` trick). A second defect beside it: a materialised module's ownership record was
replaced by its live attribution, which is only its doc entry.

## 2026-10-22 (evening) — typed parameters (ADR-420)

`(defn area (w:number h:number -> number) …)`. The planned `((x int) -> int)` was impossible —
list patterns and `&optional` defaults already own that shape — and a first macro-only `defn`
rewrite was reviewed and dropped before landing: it left `fn`, the CST tools and the checker's
raw passes binding `x:int`, and declared `any` for untyped positions, so typing one parameter
erased the inferred return. What landed strips the types in the compile pass every engine
shares and registers the `(sig …)` they spell, with `_` for an open position; the checker
keeps inferring an open result (and found two loops on the way: the same-file fixpoint never
saw an open name settle, and the body was checked against its own in-progress return). The
in-language test file was run once locally, by mistake, before the suite reached CI.

2026-10-22 (night): std migrated to typed parameters — 635 `defn` + `(sig …)` pairs in 82 files,
by a structural rewrite that pairs each top-level sig with its definition and refuses anything
that does not line up position for position (18 kept their `sig`: overloads, a compound type on
an `&optional` slot, arity mismatches). Every std module was loaded under the pre- and
post-migration binaries and every declared signature dumped: none of the 705 differs or is
missing.

## 2026-10-22 (night) — `Counted`, and the seq protocol is complete

The ROADMAP's "Prim1 half" item had been done since 2026-07-29 (`e29ffa39`: `first`/`rest`/
`empty?`/`nth` read a record's `Seqable` view on the cold builtin fallback) and was never ticked.
What remained was `Counted`: an ability with no default whose `->count` `count` and `empty?`
consult for a record before walking its view. `queue` answers from its size field (O(1));
`multimap` sums its value lists without building the `[k v]` view. The test record's `->seq`
raises, so a `count` that walked the view would fail it.

2026-10-22 (night, later): stateful transducer stages (ADR-421) — `seq/xtake`, built on a
per-run table that `transduce` owns and drops on every exit, with `seq/stage-slot` for a stage
of your own. The completing arity ADR-376 sketched was rejected: it reaches the caller's own
reducing function, and `(- acc)` negates. +0.45 µs per `transduce` call.

2026-10-22 (night, last): the "rope-level grapheme cursor" was sized against the editor and
turned out to be a correctness bug first — `std/editor` and the line editor moved and deleted by
code point, splitting clusters. Boundary primitives for ropes (`GraphemeCursor` over the rope's
chunks, ~0.3 µs a step at 389k chars, no cache) and strings; the buffer and line editor step by
them (ADR-422). Found on the way: a module function named like a root macro (`spy`) is accepted
but every bare call expands the macro — recorded as an open design question.

2026-10-22 (night, examples): 211 executed docstring examples across 38 std files, written by five
parallel agents over disjoint module groups, each verifying its own with `doctest/failures-under`
and limited to deterministic, environment-free functions. Gated here: a whole-image
`doctest/failures` is empty from the repo root and from `/tmp`, and every local gate is green.
`audit/exampleless` (all modules loaded) 834 → 676. One docstring was wrong and is fixed:
`explain/error` returns the code it looked up for an unknown code, `:unknown` only when none
could be derived.

2026-10-22 (night, illustrations): docstring examples gained a second kind — ` ;=> `, an
ILLUSTRATION for a function whose result depends on the world. Not evaluated, but doctest reads
its form and fails it when it does not read or calls something undefined, so a rename is still
caught; the example audit counts it. A line starting with `;` stays prose (the continuation
style `reflect/source-deps` already used).

2026-10-22 (night, illustrations campaign): ~550 docstring examples across the effectful std
surface (I/O, network, processes, tooling, the editor), by four agents over disjoint module
groups — illustrations where the result depends on the world, executed examples wherever a
deterministic one existed. Gated here: whole-image doctest 0 failures (repo root and /tmp), every
local gate green; two executed examples that depended on the doctest process's own state
(`eval-server/baseline-globals`, `release-holds-of`) became illustrations. `audit/exampleless`
674 → 120, the rest being Rust builtins and generated record accessors.

2026-10-22 (night, flake): CI on `f0da246d` caught `buffer_registry_test`'s "registry-stop ends
every process" failing once (`(proc/info a)` not nil) and passing on retry. `stop-buffer` returned
on the buffer's REPLY, which the process sends before leaving its loop, so "stopped" could read a
process still alive. It now monitors the process and returns on its `[:down]` — and answers a
process that was already dead at once instead of after its 2 s timeout.

2026-10-22 (night, partial rebuild copies): `%image-write` copies an unchanged section's bytes
(`[name [:copy offset length]]`, macro names from the old footer), so `nest run` after an edit
no longer materialises the unchanged modules to write them again: 2.7 → 0.30 s and 720 → 265 MB
on the 302 × 3k rig. `load-sources-cached` takes `:lazy` for that; check and test still load
everything. ADR-419 amended.

2026-10-23 (ADR-423, macro shadowing): a module's own definition now wins over a root macro
inside that module — `(defn- spy …)` makes a bare `(spy …)` call it; `/spy` reaches the macro.
What made that safe: when a macro is defined, its templates' call heads that name a root binding
are written `/name` automatically, so a module's own `when` no longer captures `when-let`'s
expansion (the reason the first attempt was reverted). Defining a root macro's name warns unless
the header says `(:shadow name)`; fuzzy, os and node declare theirs. Breaking: `macroexpand`
output shows `/when` etc.

2026-10-23 (vcs is git's and the registry's): `std/vcs` names no VCS but git. mur's row and its
client glue (`vcs/mur-program`, `vcs/use-mur-seat`, `vcs/mur-env`, `vcs/mur-files` and the rest)
are gone from std: bedit already puts mur's row itself (its `murcore`), and nothing downstream
called the std ones (unreleased, so no ledger rows). A row's hook may now be a function as well
as a symbol — a package's module names are rooted under it, and a function calling its global
by name is still late. `vcs` stays in std, self-contained, with a ROADMAP item to move it to a
package if it ever should. Also fixed the other red `a9423365` left: `format_changed`'s
fallback test reads the new "not in a version-controlled checkout" line.

2026-10-23 (package templates, ADR-424): std no longer carries a framework's starter projects.
`nest new --template hatch|web-api` (~250 lines of Hatch-specific string concatenation, plus a
table of sibling `:path` checkouts a fresh machine did not have) moved to hatch as plain files
(`templates/app`, `templates/api`), declared by a new manifest key, `:templates`. `nest new NAME
--from SOURCE` takes a path, git URL or registry name, reads the manifest as data and copies the
files, filling `{{name}}` and `{{package}}` — the second is the dependency that reaches the
package the way `--from` named it, which is what retired the sibling-checkout problem. Also:
the eval server's own node is `eval-sandbox-…` rather than `bedit-sandbox-…`. Tests not run on
this machine; the new `scaffold_quality` cases (a fixture package's template formats, tests and
runs; an undeclared template writes nothing; `--template hatch` points at `--from`) run in CI.
2026-10-23 (robustness pass): closing the gaps the last two days left.
- **Partial image rebuild:** `partial_image.rs` gains a chain of four copied images followed by a
  whole-project load (`--check-boot`), and four concurrent `nest run`s after an edit, three
  rounds — every answer equals a fresh build, and the image left behind is current. The chain
  case reds when the copy reads one byte off. The record beside the image is now written with
  the new `file/spit-atomic` (temp file + rename): a plain `spit` let a concurrent reader see 81
  torn reads in 200, `spit-atomic` 0 (`file_test`, sabotage-verified).
- **Macro capture:** the capture gate now scans ARGUMENT positions too, and every std macro, not
  only the prelude's (ADR-423 escapes heads; arguments are by hand). Nothing found; a fixture
  case proves the scans see each shape.
- **Illustrations:** doctest checks the argument count of a ` ;=> ` illustration against every
  arm of the callee, so a rename or reorder that leaves one stale is caught. New
  `reflect/arglists` (every arm; `arglist` shows the last). No stale illustration in std.
- **Shadow warnings** (import and root macro) are written through the dynamic `*err*` port, so
  `with-err-str` captures them; the macro one is queued during resolve and flushed by `compile`.

2026-10-23 (making the session's snags hard to repeat):
- **The pre-push hook rebuilds a stale binary** before judging (`make release`; a `-dirty` build
  carries HEAD's sha and still predates the edits after it) — four false reds in two days were a
  binary older than the tree. It also greps the added lines for CI's two Rust source gates that
  reddened main today (an unlisted process-wide `static`; a checker rule head that names nothing
  bound), and a prelude change runs `bare_names_test` and `prelude_capture_test`. Each new gate
  sabotage-checked on a planted violation and on a clean one.
- **`make ci-wait [REF=…]`** waits for CI's verdict on a push, following the newest run whose
  commit contains it through cancellations, and prints the failing jobs and tests.
- **The bare-name ledger** rejects a name listed twice and a group whose header count is not its
  bullet count — it found `dynamic (73)` heading 74 names.
- **The stdimage builder** names a `defdyn` in a std module as such ("declare it in std/prelude/")
  instead of blaming an already-loaded module.
- `EnvId::GLOBAL`'s doc says it is not the global scope during a source boot. Aliasing it inside
  `env_get` was tried and broke the source boot (some of the 122 sites rely on the miss), so
  the trap is documented rather than removed.

2026-10-23 (KI-286, guest standing is the link's): a review found the guest filter (ADR-415)
keyed by node name and looked up per frame, so a guest's reader still draining frames it had
read — after a full link reclaimed the name, after `node/disconnect`, or after a second
connection of the guest's own retired the first and closed — admitted them as a full peer's.
The standing (`guest::GuestLink`: grant + reachable pids) is now made once in `establish`, held
by the link's `Conn` and handed to its reader; `GUESTS` and `GUEST_COUNT` are gone, and `route`
notes pids on the link it enqueues on. Found by reading, never observed; the guards are written
but not run or sabotage-verified here — that is the test machine's. Open, not fixed: a guest can
claim a connected full peer's name and take its link.

2026-10-23 (KI-287, a guest's name is never a full peer's): a guest's node name is only its
`Hello` claim, and `register_link` treated it like any other — so a guest naming itself after
a connected full peer with a new incarnation retired that peer's link and took its traffic.
Binding a node name into the grant does not fit (the grant has no node name, `:as` is an
identity, the session cookie serves many generated names), so the rule lives in
`register_link`: a guest is refused a name a full link holds or ever held here
(`FULL_PEER_NAMES`) or another grant's guest holds; a full link always displaces a guest; a
dial and the mesh ignore guest links. Guards written, not run here.

2026-10-23 (one pair, one position, KI-288): an audit of the VM body cache — it keys a compiled
closure on the identity of its first body form — found three ways one pair reached two code
positions. The counted range loop put the fold literal's body in the loop uncopied beside the
fallback `fold` (harmless: the two scopes differ only in gensyms); the linear-map rewrites
rebuilt every list but returned a `(quote …)` as it was, so a body opening with `'tag` gave the
table-built copy and the unrewritten one the same cache entry (each then runs the other's body);
and the expansion walk returned quote forms as they were, so a macro splicing one quoted argument
into two bodies did the same. One helper, `copy_form`, now copies whatever a rewrite places twice,
and the walk gives a quote form cells of its own. Found by reading, not observed; the guards
(`one_pair_one_position.rs`, a `linmap_soundness_test` case, `vm_body_identity_test.blsp`) were
not run on this machine and still need running and sabotaging on the test machine.

2026-10-23 (ADR-425, a guest's node name is its host's): KI-287's check could not refuse a
name that is a full peer's elsewhere in the cluster and has never linked here. Rather than a
reserved namespace the client must claim in (a new client, and names still the guest's to
choose) or a name assigned in the handshake (the responder only knows it is a guest after both
`Hello`s, and the guest would have to rename itself), the host mints `guest~<n>.<incarnation>~
<claim>` for each guest link and translates the guest's pids between the two spellings at the
link — no wire change, every existing client works. No full node may use `guest~`, so the
collision is gone by construction, and `guest_may_take` / `FULL_PEER_NAMES` with it.
`node/peer-grant` adds `:claimed`. Guards written, not run here; the sabotage for each is in
KI-287.
2026-10-23 (regression found by the benchmark refresh, fixed): ADR-423 (`f9de1ee5`) cost every
program that loads a std module ~16M instructions at load, plus background JIT work — `reduce`
+20%, `pipeline` +11%, `base64` +9% in instruction counts; `make ab --floor` agreed. Bisected by
instruction count (with the boot cache warmed per step: the first unwarmed bisect measured cold
boots and named nonsense). Cause: the source boot compiles the prelude on the builder heap, whose
global scope is a frame, and `macro_head_id`'s root-escape branch asked `EnvId::GLOBAL` — so
every `/when`-style head ADR-423 put into a prelude function's expansion stayed unexpanded, and
those closures were deferred to the tree-walker for good (and baked into the prelude image that
way). It asks `heap.global()` now: loading `path` 108.6M → 93.6M (pre-ADR-423: 92.8M), runtime
`<anon>` deferrals 20 → 1. Guard: `boot::builder_scope_tests` compiles `(/mm 5)` on a builder
heap through `compile` (sabotage-verified: `(mm 5)` comes back unexpanded).

2026-10-23 (a project module a dependency also declares is refused): mur-forge named its server
module `web/endpoint`, which hatch also ships; every dev run and test passed (dev roots the two
apart), and the release did not boot — a bundle keeps the root project unrooted and loads a
uniquely-named dependency module at root too, so both bound `web/endpoint/serve` and hatch's
won. The package guard every verb runs (`package-guard-collisions`) now also refuses a root
module any dependency declares (`main` excepted), naming both, so `nest check`/`test`/`run` fail
the way the release would. No sibling project trips it today (scanned against each `_deps/`).
`docs/packages.md` § Namespace collisions rewritten — it still said namespaces were not rooted.
Tests not run on this machine; two cases in `tests/package_test.blsp` run in CI.

2026-10-23 (KI-286 / KI-287 / KI-288 guards sabotage-verified, on GitHub Actions): this desktop
runs no tests, so a throwaway branch carried one patch per sabotage and a workflow whose jobs
passed only if the guard went red, beside a control job with no patch. Every listed sabotage
went red where the entries said, on `7ed008cf` and again on `e9e1af8b` after ADR-425; KI-286
and KI-288 are closed with the red output in their sections. KI-287 stays open on one guard:
the distribution case ADR-425 extended is red on main without any sabotage — its host script
calls an unbound `nodes` — so its sabotage proves nothing yet. Not predicted, recorded: the
fused-pipeline case of `one_pair_one_position.rs` is red under none of KI-288's sabotages. The
same run A/B'd the day's perf batch against `32e71066` with a base-vs-base control,
indicative only (a shared runner): nothing regressed at the default tier; at tier 1 `collatz`
−10.3% and `fib` +3.7% (floor 1.5%, control +0.8%) — compute-frontier.md §7.8's addendum.

2026-10-23 (ADR-426, the parallel cold load): a cold project build loads its modules in a pool
of processes — each `require-one`s modules off the driver's queue, and a dependency another
worker is loading is waited for by `require-one`'s own claim/park. 302 × 3k rig on 12 cores:
load 8.5 s → 2.1 s, cold `--check-boot` 12.9 s → 6.5 s, warm runs unchanged. Getting it to
build the SAME project as the sequential walk took five fixes, each pinned by
`crates/nest/tests/parallel_load.rs` against `BROOD_LOAD_SEQUENTIAL=1`: no root-section
require edges (a warm start would replay them and load everything), no `dev` module in the
build, `*features-waiting*` kept out of the image, a failure reported as the sequential walk
reports it, and KI-289 — positions lost on code re-homed after a RUNTIME aging flip, a runtime
bug any concurrent load could hit (`crates/cli/tests/rehome_keeps_positions.rs`).

2026-10-23 (image write): profiled the cold build's image step after ADR-426 — the kernel's
encode is 0.6 s, the Brood section grouping was 1.13 s (`project-image-section-of` split and
re-joined 208 000 names). One `%str-last-index-of` + substring: image step 1.8 s → 1.1 s.

2026-10-23 (examples, finished): `audit/exampleless` 120 → **0** with every module loaded. The
Rust builtins' doc strings carry illustrations (seven run as executed examples); `defrecord`
now GENERATES an illustration in each constructor and accessor docstring, so every record —
std's and every future one — is covered; the ability ops take theirs from the ability's
docstring. Whole-image doctest 0 failures.

2026-10-24 (KI-290, a require cycle across processes): the bedit downstream smoke went red once on
`14232c96` — 13 tests `unbound symbol: ed-layout-now` in `aside`, which does `(:use panes)` — and
the build before them printed `[require] BUG: waited 30s for an in-flight load of 'bedit/panes'`.
The parallel cold build (ADR-426) had split a require cycle across two workers: each held one end
and waited for the other, so neither load could finish, and the backstop then loaded `panes` a
second time while the first was still in flight. A waiter now walks the wait-for chain every
250 ms; the process that finds itself on it raises, its claims are discarded and released as that
unwinds, the other process finishes the cycle on its own — as one process would — and the
raiser's outermost load requires its module again. The driver also reloads such a file
sequentially should the error reach it. Guard in `tests/modules_test.blsp` (two fixture modules
that pause after their claim, so the cycle forms every run); not run on this machine.
2026-10-24 (ADR-426 made opt-in): CI's downstream bedit job went red after the parallel load —
bedit's pool-built image differed from a sequential one in 89 of 116 module sections, and 14 of
`mdpreview_test`'s 19 failed on a warm run (reproduced locally on bedit's pinned commit; the
sequential build passes). The rig and every record had agreed — the rig's modules read nothing
at load. Hypothesis: load-order dependence (a module computing load-time values from state other
modules write). The pool is `BROOD_LOAD_PARALLEL=1` until that is pinned; the default build is
exactly the pre-ADR-426 sequential walk again. The throwing-module test's failing module moved
to the end of the chain (CI timed it out: a failure deep in a chain is slow — ROADMAP).

2026-10-24 (ADR-426, the second finding): the parallel pool also DEADLOCKS — a parallel build of
bedit hung in 2 runs of 4. Cross-process require cycles: in one process a module requiring
itself back mid-load proceeds; across workers each holds one claim and parks on the other's.
The flag is marked experimental; the design that could make the pool safe (schedule by the
header require graph, one worker per strongly-connected component) is recorded in the ADR.

2026-10-24 (a failing module load, the proper look): the slowness ROADMAP recorded (a 13-deep
require chain whose bottom module throws: 3.4 s to report) was exponential, not a compile cost —
8 deep 0.16 s, 13 deep 3.8 s, 20 deep still running after five minutes at 4.7 GB. Counting the
bottom module's loads gave 2^(depth−1): `%require-header-deps` loads a header's dependencies
before the source (so a wait parks) and swallows a failure for the header to raise "in context",
and the header raised it by loading the dependency again, at every level. The pass now records
the failure in `*require-failed*`, bound around the one source load, and `%require-one` raises the
recorded error; a later require, outside that load, tries again. Loads once at any depth (13 deep:
0.05 s). Guard: `tests/require_failure_test.blsp` (512 and 1 024 loads with the branch removed).
Seen on the way, not fixed: a user `error` rethrown by a catch-cleanup-rethrow (`finally`, the
loader's journal) loses its position — `catch` binds a user throw's bare payload — so a build
error from `(error …)` in a required module is reported at the outermost requirer's header (KI-291).

2026-10-24 (KI-291, ADR-427): a rethrown user error keeps where it was raised. `catch` binds a
user throw's bare payload, so every cleanup-and-rethrow raised it afresh at the rethrow site; a
module's `(error …)` at load was reported at the outermost requirer's header. A catch now records a
heap payload's origin per process (rooted for the collector) and `throw` of that payload takes it
back. Guards `tests/rethrow_position_test.blsp` (all tiers, GC stress; two sabotages) and a
rendering case in `crates/cli/tests/rethrow_rendering.rs`. `errors` row +1.15% instructions
against a 0% control, wall time within noise.

2026-10-24 (KI-290 closed; KI-281 seen again): KI-290's guard was sabotaged on a throwaway CI ref
(`%require-cycle?` never true) and timed out in both suites that run it, each fixture process
waiting on the other's module and nothing else red, so it is closed. A second throwaway ref
switched the parallel cold load back on by default over that fix, and bedit's downstream smoke
passed on it. Its tree-walker job met KI-281 for the second time: the dump shows the caller's IC
entry linkable and both arms native, so the link's gating is ruled out and the question is what
the native caller runs instead. The dump now prints whether each entry links at that moment and
whether each arm's inlined upgrade is installed.

2026-10-24 (KI-281, a race in the test): the third occurrence, on `main` with those fields,
named it: the entry was linkable and the caller ran its inlined upgrade, which splices the leaf
callee in and makes no call. Only the small native publishes the mirror, so an upgrade
installed before its first call after the callee went native leaves nothing ever published.
That is correct (and faster), and permanent. The link tests' helper (`tiered_pair`) now
discards such an attempt and retries in a fresh interpreter, up to five times. The dump's
upgrade field reads the code pointers, not `inline_installed`, which
`jit_tier_frame_pairing` keeps to an allowlist (the first cut tripped it). Not run on this
machine.

2026-10-24 (review cycle 1: KI-292, KI-293, KI-294): four read-only reviews of the week's
riskiest commits — ADR-427's record, KI-290's cycle break, the parallel cold load, the GC and
process perf commits (the last clean). Three defects. KI-292: ADR-427 recorded shared payloads,
so a dropped raise of a literal or `def`'d sentinel lent its origin to a later, independent raise
of it; only LOCAL payloads are recorded now. KI-293: the header pre-pass swallowed a
cross-process cycle error, so the backing-out process met the cycle again at every header level
(2^depth waits, module bodies re-run, a deep chain past the 30 s backstop); it rethrows now.
KI-294: the parallel driver counted workers by message order and reported a dead worker as a
missing file; it tracks pids, and a sequential pass over whatever the pool left unloaded is the
build. The first two guards sabotage-verified locally (red, the cycle case at exactly the 5 runs
the trace predicted), the third by construction.

2026-10-24 (review cycle 2: KI-295, KI-296, KI-297): three more reviews — leads left by the
first cycle, the guest-link commits, the week's macro and package fixes. KI-295, the worst:
pipeline fusion, on by default, renamed a stage's parameters through lists only, so a parameter
inside a vector, map or set literal stayed behind — unbound, or silently the outer binding of the
same name. KI-296: a guest's message could carry a pid it guessed, which became reach when echoed
back, and posed as a full peer to `serve`'s identity check; a guest now sends only its own pids
and ones it was given (ADR-425's "Not done", amended). KI-297: a require waiter that missed its
claimant's pid had no death watch; it takes one each tick. Refuted: the stale-cache read in
`%require-await` (ordered by the registry lock) and the macro `GLOBAL` lookup on a builder heap
(no namespaced module is ever compiled there).

2026-10-24 (ADR-426 the default again): the parallel cold load is on unless
`BROOD_LOAD_SEQUENTIAL=1` (the opt-in `BROOD_LOAD_PARALLEL` is gone; the new name matches
`BROOD_CHECK_SEQUENTIAL`). Both reasons for the opt-in are gone. The cross-worker deadlock was
KI-290, now closed and sabotage-verified. Since KI-294 the pool is a prefetch with a sequential
pass behind it. With the pool on by default, bedit's smoke passed 3 attempts of 3. Not run on
this machine.

2026-10-24 (ADR-428, `std/settings`): a setting is declared once — `(settings/define id name
{:default :type :doc …})` — and its value check, accepted-value text, warnings and documented
default file all derive from that declaration. nest's user config is the first caller: its three
keys are declared, the default file is generated, and a rejected value (`:git-init "no"`) is
warned about on stderr and not applied. bedit's init file is the intended second caller.
Guard: `tests/std_settings_test.blsp`. Not run on this machine.

2026-10-24 (ADR-429, fuzzy terms): a space in a fuzzy query separates terms that must each
match, in any order, scored independently and added; one term ranks exactly as before. The
native `%fuzzy-top` splits and adds the same way (`score_terms`), and
`tests/fuzzy_native_test.blsp` holds the two equal on multi-term queries. Not run on this
machine.

2026-10-24 (ADR-430, `std/sorted`; KI-298): a sorted map and set, ROADMAP's "other Lisps" item 4.
The tree is a treap whose priorities are the keys' structural hashes (a new `%hash` primitive
over `Heap::hash_value`), so its shape depends only on its keys and two collections with the
same contents are `=` whatever order they were built in. It has min/max, floor/ceiling and a
half-open range, and it is a full collection (`Seqable`, `Counted`, `Conjable`, `Lookup`).
Writing its `get` turned up KI-298: `(get coll k default)` with a non-nil default never asked
`Lookup`, so `(get multimap :k [])` gave `[]` for a key holding values. Fixed in `get`'s
3-arity arm and in `PrimOp3::MapGet3` alike. Not run on this machine.

2026-10-24 (ADR-431, `seq/iterate`): an unbounded source that only `seq/transduce` consumes, ended
by `xtake`/`xtake-while`/a stage's `reduced`. Realising it raises instead of looping. A `seq/l*`
view over one hands its stages to the run. No lazy value and no prelude change. Not run on this
machine.

2026-10-24 (ADR-432, `next-impl`): an `impl` method can call the impl it shadows, one precedence
tier down, else the ability's `:default`. Registration now keeps every tier (`*impl-tiers*`),
and both images replay all of them, so a warm start reaches the same shadowed impls a cold
start does. A method that never mentions `next-impl` is registered as written. Not run on this
machine.

2026-10-24 (CI red on 87987990, both defects mine): `std/sorted` recursed forever on an empty
collection. Its nil `%tree` field read as a miss, `get` handed it to `Lookup`, and the impl read
the tree again. It reads its own fields with `%map-get` now. The KI-298 `get` change is
reverted: `linmap_soundness_test` pins "a default never reaches `Lookup`" on purpose, and the
linmap loop path keeps that rule. KI-298 is a design question now, not a fix. Not run on this
machine.

2026-10-24 (ADR-440, the GPU target's feel): the installed build paints on the GPU, and that
path re-shaped every text run and re-uploaded every instance each frame. Now a run's quads are
kept and replayed while the run repeats; position is not in the key, so a scroll reuses every
line it keeps. The instance buffers write only the span that changed. Translucent geometry
blends in linear light through an sRGB view; text keeps the encoded lerp and gains the
contrast lift. The default present is MAILBOX (else FIFO) with one frame in flight;
`{:vsync false}` is the only road to IMMEDIATE. A window stays hidden until its app's first
frame is painted, or 500 ms. `BROOD_GUI_TRACE=1` prints input → present per frame, and
`gui/latency` returns the samples. The cursor op takes a colour and a `:hollow` style. Built
with and without `gui-gpu`, clippy clean. No tests were run on this machine and no window was
opened: the reuse and present-mode tests are for CI.

2026-10-24 (ADR-441 the GUI keyboard): a window's input is the user's. `{:key-repeat :os}` hands
a held key's repeat to the OS (each repeat a press, gated by a mailbox delivery credit so at most
one is ever queued; `:app` stays the default). Super (`:super-x`, `[…][shift-][super-]<name>`),
F1–F24, `:insert`, `:menu` and the other named keys, in both frontends. A shifted chord takes the
layout's glyph, not the US table. `ke.text` for typing, `[:text …]` for multi-character text and
IME commits, `[:preedit text range]`, the IME candidate window at the frame's cursor, `:focus-in`
on focus gain. `{:wheel :lines}` — three lines a notch at once — is the default; `{:wheel
:glide}` keeps the glide. Built and clippy-clean with `gui,gui-gpu`; the tests were written but
NOT run on this machine, and no window was opened.

2026-10-24 (ADR-442, fonts): the GUI finds installed fonts by name. `gui-font-register` takes
`{:family-name "JetBrains Mono"}`; the system font database is scanned only on that first ask,
or on the first glyph nothing loaded covers. A CJK ideograph or a missing symbol now comes from
an installed font found by coverage, where it used to be tofu; emoji still come from the bundled
Noto. `gui-ligatures!` (off by default) shapes 2–8-char punctuation runs as one cell-spanning
unit in both painters. `gui-font-info` says which file each style resolved to. Rust tests
(`render.rs` `font_tests`) were written for CI and were not run on this machine; nothing was run
here beyond the gui / gui-gpu builds and clippy.

2026-10-24 (ADR-443 tree handles): tree-sitter parses incrementally from a tree the caller
KEEPS. `%tree-sitter-tree` parses a whole document (a rope, read through tree-sitter's chunk
callback, never flattened) into an immutable ref handle; `%tree-sitter-tree-reparse` edits a
copy of its tree and reparses (explicit char edits, or derived by a chunk-wise prefix/suffix
compare of the two ropes); `%tree-sitter-tree-spans` colours a char RANGE of the held tree
without parsing; `%tree-sitter-tree-changes` reports the edit plus tree-sitter's changed
ranges, so a span cache can invalidate by range; `%tree-sitter-tree-free` drops one. The
registry is bounded (32, LRU); a gone handle reads nil and reparses as a full parse.
`editor/treesit` wraps them (`open-tree`, `update-tree`, `fontify-tree`, `tree-changes`,
`free-tree`, `splice-edit`); `fontify` and every slice-based primitive are unchanged, and bedit
is not yet moved onto them. Rust tests and a reparse-equals-fresh-parse test written; **no
tests were run on this machine** (built, clippy clean, `nest check --strict` clean).

2026-10-24 (ADR-444, `ui-run` skips no-op turns): the loop remembers the model and grid it last
drew, and a turn that leaves both `=` renders nothing. In bedit that is any timer that found
no work and a release that changed nothing. (A release that ends a held key, and the hosted
buffer's echo of an edit, still render: each changes the model, though the view they build is
all memo hits.) Not run on this machine.

2026-10-24 (ADR-433, `gen/start` options): `gen/start-link` and `gen/start-named` are gone;
`(gen/start f state {:link true :name :k})` composes the two, and an unknown option raises. The
rename ledger points at the new call shape, and `--fix-renames` leaves a call-shape target to a
human instead of rewriting it to a bare name and dropping the link. Not run on this machine.

2026-10-24 (ADR-434, `& rest` in pattern clauses): a pattern-dispatched `fn`/`defn`/`defmacro`
clause may take `& rest`. Its head is a list pattern over the arguments, where `&` already
meant the rest; `fn` had rejected it alongside `&optional`. Macros can now take one clause per
shape and end with a catch-all raising their own message, which is what ROADMAP's `:hint` was
for, so no `:hint` syntax. Not run on this machine.

2026-10-25 (ADR-435/436, `defapp`): `os/trap` delivers SIGTERM, SIGINT or SIGHUP sent to the
runtime to a process as `[:signal sig]` (a self-pipe and one reader thread; a dead subscriber
restores the default action). `std/proc/application.blsp` adds `defapp`: `:requires` start
first, each once, and everything stops in reverse on a signal or `application/shutdown`. When
`:main` names an application, `nest run` and a release run it, and `--check-boot` starts and
stops it. Not run on this machine: `tests/application_test.blsp` and
`crates/cli/tests/application_sigterm.rs` are new and run in CI.

2026-10-25 (regex conformance corpus, ROADMAP corpora item 4): `tests/corpus/regex` vendors
rust-lang/regex's Fowler testregex files (345 cases, expectations for leftmost-first, which is
Brood's semantics). `tests/conformance_regex_test.blsp` reads their TOML subset and compares
every first match and group span with `regex/find`. Cases that differ because of Brood's
dialect (no case-insensitive mode, no POSIX classes, `\n`/`\xHH` are letters) are listed by
name with the reason, and the test fails if one starts agreeing. Not run on this machine.

2026-10-25 (ADR-437, `defstatem`): `gen/defstatem` defines a state machine over the gen
envelopes. Each state is a block of `cast`/`call`/`info`/`timeout` clauses returning
`[:next state data opts?]`, `[:keep data]`, `[:postpone]` or `[:stop data]`; an `any` block
applies everywhere; `call` binds `reply` for `gen/reply`. A state timeout is cancelled by every
transition and carries the transition count, so a late one is dropped. The macro writes the
`match` dispatch, and the loop is `gen/run-statem`. Not run on this machine:
`tests/statem_test.blsp` is new and runs in CI.

2026-10-25 (regex corpus findings): the Fowler corpus found a real bug on its first run.
`{m,n}` was expanded into copies of its atom at parse time, which copied a capturing group
too: `(a*){2}(x)` reported three groups, and every group after a counted repetition was
misnumbered. Counted repetition now reaches the engine as `{m,n}`, which keeps one group and
the last iteration's capture (44 cases). Two more cases differ by design: a `]` first in a set
closes it (`[]` is the empty set). Also `os/trap`'s pipe uses `pipe` + `FD_CLOEXEC`; macOS has
no `pipe2`.

2026-10-25 (review of ADR-435..437): lifting a trap, or a dead subscriber giving one up,
set the signal to `SIG_DFL`, so a `nohup`'d program that trapped SIGHUP died on its next
hangup, and the REPL lost its Ctrl-C handler. The first trap now saves the disposition and
both paths put it back, under a lock that also stops a trap racing the reader thread
(`crates/cli/tests/application_sigterm.rs` starts a runtime with SIGHUP ignored).
`application/run` hands each signal back to whoever trapped it before, rather than to
nobody, and drops its root monitors before stopping them; its crash test is `:isolated`, as
it traps the test runner's own signals. `defstatem` refuses a machine with no data
parameter, and a backwards `{3,1}` is literal text as the regex dialect's malformation rule
says, rather than an engine error. Not run on this machine: every test, and the new ones
run in CI.

2026-10-25 (simplification, no behaviour change): a signal's subscriber and its saved
disposition are one `Trap` behind one lock (`SIGNAL_TRAPS`), so the reader thread no longer
needs a compare-and-retry against an atomic. `application/start` is a fold over the start
order, which is a plain vector (the `[seen order]` pair and `start-each` are gone), and
returns a vector. `defstatem`'s loop takes the next state straight from `receive`, without
the `:stop`/`:stale` sentinels. `project-entry` resolves an entry point once for
`project-entry-fn` and `check-boot`, replacing the near-copy `project-entry-application`.

2026-10-25 (ADR-436, the two boot checks): `nest run --check-boot` starts and stops an
application `:main`; a release's boot check resolves it and says `an application, not
started`, because it runs on the build machine. That difference is now the rule, written in
ADR-436 and in both checks: they resolve through one public `project-run/project-entry` and
print through one `project-run/boot-ok-message`, so the start is the only step between them.
New guards: `--check-boot` prints the starts and stops in dependency order and fails on a
start that raises (`crates/nest/tests/boot_check_and_renames.rs`), and a bundle's boot check
never starts the application the same binary starts when run
(`crates/cli/tests/release_bundle.rs`). Not run on this machine.

2026-10-25 (`count` and `(nth v i default)` inline; KI-301): profiling `json` found that a call
to a small prelude function cost ~2 000–3 000 instructions from native code, whatever it did:
`count` of a four-element vector 3 100, `(nth v i -1)` 1 200, `seq/subvec` ~6 000. `count`
becomes `PrimOp1::Count` (`Heap::count_inline`: vector, string, range, bytes, set, nil, non-record
map, proper list; anything else dispatches the Brood `count`) and `(nth v i default)`
`PrimOp3::VectorRefOr` (`Heap::nth3_inline`), each a VM arm plus a JIT callback that deopts on a
decline: 3 100 → 220 and 1 200 → 250 instructions. Removing the call also lets an arm whose only
call was one of them past the call-mediated gate. KI-301: the JIT prepass counted a staged call
(KI-19) as unstaged, so every arm with one before a join bailed — `json`'s number scanners among
them. `std/json` reads with `(nth s i -1)` (the `json-at` helper is gone) and slices a string with
no escape instead of consing and reversing it; `seq/subvec` has two fixed arities. `make ab --floor`
against 260c43bc: `json` −20.8%, `regex` −11.4%, `base64` −13.6% in one run and −2.5% in the next,
nothing past the gate. `pingpong`/`ring` read +3% at equal instructions. `make ab-vm` read
`pfib` +11% at tier 1: equal-or-fewer instructions, identical when pinned one thread per core, and a
same-source control build moves the µop-cache counters further than this change does, so it is code
placement under SMT, not work. Also found: KI-302, `startup` +3% from `7acef6e2`, left open. Not run
on this machine: `tests/count_nth_inline_test.blsp` is new and runs in CI.

2026-10-25 (KI-302 fixed, and its first diagnosis retracted): the `startup` +2.56M from `7acef6e2`
was not the third registry write per `impl`. Startup replays 8 staged registrations, and dropping
every staged epoch bump saved 0.42M. It was a hash-table doubling. Rebuilding the prelude image
re-stamps 7 324 source positions into a `form_pos` table grown from empty, and `next-impl`'s four
prelude functions took it past 7 168, one more full rehash on every start. The prelude image header
(v5) now carries the count (`message::positioned_lists`) and the load reserves it once: `startup`
87.43M → 85.26M instructions, −5.6% on `make ab --floor`. Not run on this machine: the new
`positioned_list_tests` and the suite run in CI.

2026-10-25 (KI-298 fixed: a `get` default means absent; KI-299 was the test): `(get coll k default)`
answered a non-nil default for a key that is not a record field without asking `Lookup`, so
`(get multimap :k [])` gave `[]` for a key holding values. Now a record's miss asks `Lookup` whatever
the default, and a `Lookup` that answers nil keeps it: `get`'s 3-arity arm (its variadic arm forwards
to it), `Heap::map_get3_inline` for the VM and native code, and `get-in`'s step, which had the same
blind spot through `contains?`. The October 24 attempt was reverted on the belief that the linmap
loop path kept the old rule; both linmap paths run a record seed as written (`%table-from-map`
refuses a record), so they agree by construction, and `linmap_soundness_test` pins 102 (the
ability's 100 plus two) where it pinned 2. KI-299: P16 in `breakage/chaos2_process_links.blsp`
read `:normal` because P15's child announces itself before it finishes and can die after P16
turns the trap on; P16's unpinned `[:EXIT _ reason]` took that exit. Every exit and down receive
in the file now pins its pid or ref, and P15 waits for its child's death. KI-300: the
interpreter redirects a thin wrapper to its inner head, so `jit_computed_call_test`'s wrapper is
counted only once its caller `fold-with` is native, and `settle` never waited on `fold-with`'s
compile; each thin-wrapper test settles `fold-with` first and the redefining tests run
`:isolated`. Not run on this machine.

2026-10-25 (`names`, a process registry; ADR-445; ROADMAP gap E, local half): a key that is
any value, held by one process (`:unique`) or many (`:duplicate`), with a value beside each
holder. The registry is a `defserver` owning a table (ADR-418), so writes are serialised and
its entries die with it; a via directory in `gen` points each registry's name at its table,
so `lookup`/`whereis`/`count` read without a message. A holder is monitored once and its down
drops its keys; reads keep live holders only, for the window before the down is handled.
`gen` resolves `{:via registry :key key}` in `call`/`cast`/`stop`/`code-change` and registers
through it in `start`'s `:name`. The directory is `gen`'s and `gen/start` sends the
registry's register call itself, because a `names/…` reference in `gen` made each module load
the other first (`nest format --check` found it: `defserver` unbound while `names` loaded). Cluster-wide process groups stay open. Same day: forward-facing docs, std, tests and
Rust comments describe Brood in its own terms (the August decision), `docs/elixir-parity.md`
is gone, and history files are untouched. Not run on this machine:
`tests/names_test.blsp` is new and runs in CI.

2026-10-25 (the call IC entry is 48 bytes, was 64): `CallIcEntry` is one per call site a process has
run, the largest item in a parked green process. A `spawn-live` unit holds 7. Its callee is now a
one-byte kind plus a `u64` handle instead of a 24-byte `Value` (24 for `Pid`'s second word, which no
callee needs), `argc` is a `u16`, and the arm's env sits beside the arm instead of in a tuple. Only
closures, builtins and keywords are cached; anything else, or a site with more than 65 535 literal
arguments, resolves on every call as a cold site does. The size is a compile-time assertion.
`spawn-live` at 200 000 processes: 943 → 927 MB peak RSS. `make ab --floor` (fib, pfib, bintree,
spawn, spawn-live, pingpong, ring, json) and `make ab-vm`: noise. Not run on this machine: the new
`call_ic_entry_tests`, in CI.

2026-10-25 (`groups`, process groups across a cluster; ADR-446; ROADMAP gap E done): one
group server per node, registered as `:groups`, owning a table of group → pid → origin node.
Each server monitors its own node's members and casts every join, leave and death to its
peers, so a member's origin is whoever reported it and no pid has to be asked for its node.
New peers come from polling `(node/list)` (the kernel has no node-up event), get
`node/monitor`ed and a full sync, and answer with theirs once; a node's members go with its
link. Reads go straight to the table. Eventually consistent, no unique names. Not run on this
machine: `tests/groups_test.blsp` and `crates/cli/tests/process_groups.rs` (two real nodes)
are new and run in CI.

2026-10-25 (a refused arm keeps its reason; `dev/why-not-native`): the JIT's `BAILED` latch
said only that an arm was refused. The reason went to stderr, and only under
`BROOD_JIT_BAIL_TRACE` set before the run — which is how mandelbrot sat interpreted with
nothing to say why. Every latch now goes through `CompiledArm::latch_bailed`, which keeps the
reason its trace line names (the lowering's own recorded reason, `hosts-receive`,
`suspend-latched`, `deopt-thrash-latched:<last deopt>`, …) in a `OnceLock` on the arm; an inline
upgrade's slot is latched without one, since the small native keeps running.
`%jit-arm-state` returns it as `:reason`, `dev/jit-state` wraps that, and `dev/why-not-native`
answers in a sentence, with the explanations kept in Brood. The first piece of the
predictability work. The latch itself is unchanged, and whether some latches should back off
instead is next. Not run on this machine: `tests/why_not_native_test.blsp` is new and runs in
CI.

2026-10-25 (native code reads a `def`'d vector inline): a `def` promotes its vector into the
shared RUNTIME region, and the JIT's vector reads (`vector_ref`, `inline_vec_ref`) indexed only
LOCAL vectors, so every element of a `def`'d one went through `brood_rt_vector_ref`: 31% of
`matmul`'s cycles. `shared_vec_slot` now finds the `VecStore` itself. RUNTIME objects never move
within their generation (`boxcar` is append-only), so the address is `boxcar`'s bucket base plus
offset, with bucket `⌊log2(i + 32)⌋ − 5`. Each `CodeSlabs` keeps a table of bucket bases, filled
by the callback on its first read in a bucket. The heap keeps each cached generation's
`CodeSlabs` address beside `gen_cache_ver`, and native code trusts it only while that version
equals the runtime's live `gen_version`: a replaced generation (Stage-4 free, compaction) bumps
it, and the next read takes the callback. The `boxcar` layout is never read directly. A startup
probe checks the formula against a real `boxcar::Vec`, and if they disagree every RUNTIME read
keeps the callback. PRELUDE vectors keep the callback. `matmul` −20.4% on `make ab --floor`
(1.61G → 1.21G instructions); nqueens, sieve, nbody, mandelbrot, json, regex, base64, sort, fib,
bintree, persistent-map are noise. `json` reads +1.8% instructions at equal time (+1.3% at 15
reps, inside the floor), most likely the larger code every vector read now compiles to, on the
compiler thread. +24 B per heap. New guards: `shared_vector_read_test.blsp`,
`the_shared_vector_location_is_where_boxcar_puts_it`, and
`native_shared_vector_reads_survive_a_runtime_compaction`. Not run on this machine: those, in CI.

2026-10-25 (`caar` to `cdddr` in the prelude; ADR-447): the compositions of `first` and `rest`,
two and three deep, beside `second` and `third` in `std/prelude/predicates.blsp`, for a nested
list taken apart, which was `(first (first (first x)))` written inside out. Over `first` and
`rest`, so over lists, vectors and nil. Asked for by the wos work, whose device compiler expands
each into its `car`s and `cdr`s. Not run on this machine: the new
`tests/list_compositions_test.blsp`, in CI.

2026-10-25 (one-call HOF loops are native): `matmul`'s remaining 18% in `exec_chunk` was
mostly its setup, `(into [] (map (range n) f))`, at ~3 850 instructions an element, because
`map`'s own `%defseq` loop never tiered. It bailed mid-emit at `call-spill-exhausted`: the loop
keeps `(rest coll)` on the stack across the call to `f`, native code spills such a handle to a
reserved frame slot, and `jit_spill_reserve` reserved none for an arm with fewer than two
non-tail calls. Its comment called that bail harmless, from a 2026-08-29 measurement where
widening the reserve cost `spawn` +8.6%. Re-measured alone on today's tree it costs nothing:
20 rows noise on `make ab --floor` (`spawn` −4.7%, then +0.0% at 15 reps), `make ab-vm` noise,
per-process RSS slope unchanged. The rule is now `< 1 → 0`, and the map/into probe goes 570M → 406M
instructions (−29%). Checked by a differential (tree-walker = JIT) and under
`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`. Guard: `hof_loop_native_test.blsp` (`hl-map` bailed on
the 6577bfaf binary, native here). Not run on this machine: it runs in CI.

2026-10-25 (a moved code epoch revalidates a native arm; ADR-450; KI-303): every rebind moved the
global code epoch, and a native arm entered after one reset and recompiled, whatever had been
rebound. The suite rebinds constantly — module publishes, registry writes, isolate restores — so
`shared_vector_read_test`'s inner loop read `:untried` after 6 000 calls. A diagnostic script on the
desktop (approved for this) reproduced it with one process redefining an unrelated function in a
loop: the inner loop sat at `:queued` every round. `native_still_valid` now checks what native
code baked in — relocations, inlined operators, a leaf derivation, a self-loop's name, ADR-366
staleness — and moves the stamp forward when none changed; the same script reads `:native` from
round 5. Global reads and calls re-check the live epoch themselves. Not run on this machine:
`tests/jit_unrelated_rebind_test.blsp` is new and runs in CI; a `make ab` on the test machine is
the measure of what a quieter JIT is worth on the rows.

2026-10-26 (ADR-450 withdrawn; KI-303 reopened): CI run 40237227384 on the revalidation fired the
fast-link mirror's debug cross-check (`link.rs:338`), and two JIT tests answered wrong —
`jit_self_rebind_test` 12 for 1001, because the register worker's direct self-link is a `Call`
on the arm's own name, which the check covered only for `SelfCall`; `optional_defaults_test`'s
deep loop 1 for 5000050000. The code epoch carries more than the arm's own guard, so the change
is backed out and `jit_unrelated_rebind_test` with it. The cause stays reproduced; the next
attempt enumerates every `code_epoch` reader first and runs the suite before it lands.
`shared_vector_read_test` and `receive_loop_jit_test`'s settle cases run `:isolated` meanwhile,
alone, where no other test rebinds. Not run on this machine.

2026-10-26 (ADR-450 reinstated; KI-303 and KI-304 fixed): KI-303 had two resets, not one. Besides
the native entry, the tier-up election marked its arm stale whenever the epoch moved during
`preload_arm_globals`, taking any concurrent rebind for a module the pre-load had loaded, and a
stale arm is rebuilt untried — why `sv-row-sum` read `:untried` even with the first attempt in.
The pre-load now answers whether a name the arm reads went from unbound to bound. Of the first
attempt's three regressions: the name check now covers any call by the arm's own name (the
register worker's self-link) and an inlined self-recursion; the stamp moves with `fetch_max`,
since two processes storing it could leave it behind a published fast-link mirror, and the
mirror's debug check accepts a callee stamped past its epoch; the compiler's install check counts
resets (`native_resets`) instead of comparing the stamp. The third, `optional_defaults_test`
answering 1, was a separate miscompile the resets had hidden (KI-304): the lowering's slot caches
held SSA values across block leaders, so `&optional`'s default store in the prologue's then-branch
was read after the join on every path. The emitter drops both caches at each leader. A desktop
diagnostic (approved for KI-303) with a process rebinding a name 3 million times kept the hot
arms native and the sums right; the 3333b20b binary answered 1. Guards:
`jit_unrelated_rebind_test` (restored, plus a case that settles while a process rebinds in a
loop) and `optional_defaults_test`'s native-tier accumulator case. Not run on this machine: the
suites run in CI.

2026-10-26 (KI-127's cause; an undecodable image section raises): the one silent 48-minute hang of
`autoload_race` (2026-09-11) was a racing child's death behind a bare `receive`, and the death is
the one `9de9f2d3` measured two weeks later under libtest — `unbound symbol: string/blank?` while a
sibling test rebuilt the stdlib image. KI-224's process-wide image handle (fixed 2026-10-13) made
the section read misaligned; the second half was still live: a section that does not decode
answers nil from `%image-load-section` after defining the entries before the bad one, and the
materialiser provided the module anyway. It raises now, so a misread image fails like a broken
source module and `require-one` releases the claim to its waiters. Found by reading (two
read-only review passes; no scheduler lost-wakeup path exists — every worker wait is bounded at
500 ms, and the receive park and every delivery share the mailbox lock). Guard:
`image_section_decode_test` — the 89267d84 binary provides a module whose only section does not
decode; this one refuses it, once and to eight racing processes. A desktop probe script, not the
suite; the suite runs in CI.

2026-10-26 (a wake stops reading the clock; what a park/wake round trip costs): `pingpong`'s round
trip is ~12 900 instructions, ~6 000 cycles and 82 locked instructions. The locked instructions
are not the cost: giving each allocator thread its own counter shard (a plain load and store)
took 14 of them away and moved neither the round trip nor an allocation loop, across repeats and
the whole run, so it is not in. Every wake read the clock to stamp `queued_at`, a figure only a
thief looks at: `wake_enqueue` now leaves it 0, and the first thief that looks stamps it
(`within_first_refusal`), so the owner's first refusal starts no earlier than before. A spawn or
preempted process is still stamped when queued (a fan-out's children are what thieves take). −330
instructions a round trip; `ring` −4.1% and −4.6% over two A/Bs against 8b7fddaa (best of 21),
`pingpong` 0 to −1.4%, `spawn`/`supervisor` flat; `spawn-live` read +1 to +2% wall-clock with
the same instruction and cycle counts. Also measured: each process runs `receive` twice per
message (the scan that parks and the one on resume), and the rest of the round trip is spread
over ~40 sites, none above ~3%. The send-then-receive native loop that would remove `ping`'s
bytecode was built and measured slower on 2026-10-17 (ADR-410's amendment). Guard: the
`first_refusal_tests` unit tests. Not run on this machine: the suites run in CI.

2026-10-26 (`(vector a …)` compiles to `MakeVector`; packed vectors set aside): `nbody`'s
`advance`, `(vector (advance-body b 0) … (advance-body b 4))`, ran on the VM. Its only vector came
from a call to the variadic `vector` native, so it failed the JIT's call-mediated gate. The
lowering now compiles a call whose head is the bare, reserved `vector` (1 to 32 arguments) to the
literal's `Node::Vector`. No guard: a reserved name cannot be rebound, and a module that defines
its own `vector` has its bare uses qualified to it, which this does not match (checked with a
module-local `vector` before and after). `nbody` −10.7% (`make ab --floor` against 8b7fddaa, best of
11); every other row noise across two A/Bs, `spawn` re-run at 31 reps (−2.4%, 0.0%). Packed
int/float vector storage (compute-frontier's design note, item 2) was measured first and set aside:
a `Float` is already immediate in the 24-byte `Value` (no box on read), `mandelbrot` reads no
vectors, `nbody`'s ~180 element reads a step are L1-resident (a tag check is ~1% of a step), and
`matmul`'s strided column read misses L1 at 24 or 8 bytes alike. Against that, every one of ~174
`heap.vector(id)` readers takes a `&[Value]` slice a packed store cannot provide. Guard:
`vector_call_inline_test` (values on every tier, argument order, a module's own `vector`, 32 and
33 arguments, workers across processes, and a tier case: `vci-pair` bails `call-mediated-boxed`
on the 8b7fddaa binary and is native here). Not run on this machine: the suites run in CI.

2026-10-26 (KI-305: a rebound type predicate reaches native code): a user's own
`(%eq (type-of x) :kw)` predicate is inlined as `PrimOp1::TypeIs`, and `native_still_valid`
re-checked only the 2-ary inlines, so a native caller kept the old predicate after a `def`
(3 where the VM answered 5). `chunk_ops_native` now re-checks `Prim1` and `Prim3` heads too.
The rebound arm then latches `:bailed`, as for a rebound `+`. Guard: `jit_prim1_rebind_test`;
its shape run as a script reads 3 at tier 2 on 8b7fddaa with the arm `:native`, and 5 with it
`:bailed` after. The first push's guard also asked the arm to go native again once the predicate
was restored, which the latch never allows; CI went red on it, and on the KI having no archive
section. Not run on this machine: the suites run in CI.

2026-10-26 (the editor's per-event costs: ADR-465..468): four mechanisms bedit's performance
audit asked of the language. `reflect/eval` of a lone symbol bound as a global reads the table
instead of running the compile pass, resolving the name as the pass would and declining
whenever the pass could do anything but read it (ADR-465; every late-bound keymap, layer and
hook lookup was a compile). `editor/buffer`'s splice log keeps a typing or deletion run as one
entry, with the texts it passed through so a holder that synced mid-run still gets an exact
splice, and an undo, redo or region undo logs the splice it restored, so a language server
synced after a long burst or across C-/ is sent edits rather than the document (ADR-466).
`%tree-sitter-tree-chain` / `-kids` / `-alive?` answer the structural queries from a held tree,
and `indent-column` and the `point-*` motions take an optional held tree, so the first query
after an edit no longer parses the whole file (ADR-467). `%diff-line-hunks` is Myers' line diff
with git's indent-heuristic compaction, under `diff/line-hunks` and `diff/line-status`, so a
change gutter needs no `git diff` subprocesses (ADR-468). Guards: `namespace_test`,
`buffer_test`, `treesit_module_test`, `diff_test`, and the unit tests in
`builtins/line_diff.rs`. Run here (single files, the GUI-less treesit-grammars nest): namespace, buffer,
buffer_client, diff, treesit_module, eval_vm, layers, keymap, doc_examples, doctest — all green,
and each new guard sabotaged red. Not run here: the Rust unit tests of `line_diff.rs` (clippy
compiled them), `lazy_load_test` (it needs a `brood` binary beside `nest`), the rest of the suite.
2026-10-26 (ADR-469, a caret that blinks in the window): `[:cursor row col style colour blink]`
with `editor/display/cursor-blink` (`{:interval ms :blinks n}`) hands the blink to the GUI
window. It keeps the clock on its own thread, restarts it shown on every key, click, wheel step
and caret move, holds it shown without focus, and stops after `:blinks` cycles. A toggle costs
no turn of the app's loop, and the CPU painter re-rasterises only the caret's row (its strip
diff knows a blinking leaf whose phase flipped). Guard: `paint::strip_diff_tests` (a flip
repaints one row; hidden equals the frame without the caret; scroll blits in every phase match
a full raster). The clock itself is not covered: headless makes a draw a no-op. Run here:
the filtered `cargo test -p brood --lib` (strip diff, fragments, cell regions, robustness) and
`display`, `gui`, `gui_robustness` headless; the suites run in CI.

2026-10-26 (ADR-470, memoised fragments reach the window): `ui-memo`'s marker says whether it
was reused, `gui-display` declares `:fragments?`, and `ui-run` hands it the frame with its
markers (`ui-harvest frame true` tables them without splicing). `%gui-draw` keeps a per-window
parse cache: a reused fragment that is the same heap value under the same key is last frame's
`Arc<[Op]>`, carried as `Op::Fragment`, and the frame diff, the strip diff and `prev_ops` cost
a pointer per unchanged fragment. Guard: `fragments::tests`, `paint::strip_diff_tests`,
`tests/ui_test.blsp`, all run here; the suites run in CI.

2026-10-26 (ADR-471, a line-state highlight cache): `text/changed-range` (native
`%rope-changed-range`, appended in `editor_native` for the intern-order reason) finds what
changed between two ropes from both ends. `editor/highlight`'s `line-cache-update` keeps
`[text state-in spans state-out]` per line and re-lexes from an edit's first line until the
state agrees with the old line's; a stateless lexer keeps a memo by line text.
`editor/lexer/lexer-line-lexer` adapts every table grammar. Guard: `changed_range_tests`,
`tests/highlight_test.blsp`, `tests/lexer_test.blsp`, `doc_examples`, all run here; the suites
run in CI.

2026-10-27 (a JIT-refusal sweep over every benchmark row finds no cheap lowering left): every
row run with `BROOD_JIT_BAIL_TRACE=1` on `9e5866f3`, each refusal ranked by whether its arm is
hot. None is the `nbody` kind (one call to a variadic native keeping a hot arm off the native
tier). By class: (1) `MakeClosure` in the arm — `nqueens`' `solve` (its row ~20% in the VM
loop), the spawn loops, `latency`'s `dispatch`, two `matmul` closures; the native callback was
removed as a dead lever on 2026-10-12, and the profitability gate refuses `solve` anyway
(`call-mediated-boxed`), so this class needs partial lowering (compute-frontier §2c). (2)
`hosts-receive` — by design; the send-then-receive native loop measured slower (ADR-410's
amendment). (3) `errors`' self-loop around `try` — the row's cost is the throw and unwind,
which run in the runtime on either tier. (4) `call-mediated-boxed` on prelude and std arms —
`seq`, `reverse`, `map`, `into`, `get`, the supervisor and regex internals run at setup, and
`%match-count-sym` only during macro expansion; `json-slice` forwards to two natives, so
lowering it moves nothing. (5) Zero-slot wrappers and `module-load-in-flight` retries.

2026-10-27 (the call-mediated gate admits a short arm; `regex` −6%): the JIT's profitability gate
refused every named function with a non-tail call and no inline vector or self loop
(`call-mediated-boxed`), on 2026-08-29's measurement that admitting them regressed every row.
Re-measured with the gate bypassed on `9e5866f3`: elapsed flat on every row but `regex` (−5%),
and `matmul` +5–10% CPU — the compiler thread compiling `into`/`seq`/`reverse` (~35–100
instructions, run only at setup). A native call is ~218 instructions against the VM's ~1 400
(compute-frontier §7.12), so a short arm that is mostly calls wins; a long branchy one costs its
compile. The gate now admits an arm of at most 24 bytecode instructions. Swept 0/16/24/48/all:
`regex`'s win is `regex/regex-whole-src` (21 instructions) — at 16 it is gone — and `matmul`'s
CPU is flat to 24, +5% at 48. Result against `9e5866f3`: `regex` −5.4% instructions, 34.5 → 32.4
ms; `make ab --floor` over every row, nothing flagged, `matmul` 77 → 77 ms. The cost: programs
that expand `match` forms now compile `%match-count-sym` (19 instructions, hot during macro
expansion) once per run, a constant +7–16M instructions on the compiler thread (+0.7% `nbody`,
+1.1% `errors`, +2.3% `errors-deep`; elapsed within noise; constant across `BENCH_N`). Guard:
`jit_small_call_arm_test` — a 4-instruction call arm settles `:native` (it was refused on
`9e5866f3`, checked with `BROOD_JIT_BAIL_TRACE`), a ~40-instruction one stays `:bailed` for
`call-mediated-boxed`, and both answer the same in eight workers. Risk the suite must clear in
CI: contract shims and small `receive`-adjacent helpers are this size, so the suspend latch
(compute-frontier §7.1, latent until now) and `contracts_mode`'s depth case get real arms. Not
run on this machine: the suites run in CI.

2026-10-27 (a gate for the 2026-08-06 naming scrub): `scripts/banned-names.sh`, run by CI's
rustfmt job and the pre-push hook, fails when a forward-facing file names another runtime; the
scrub had only a memory note behind it. History (the devlog, decisions, archive, changelog and
research notes) is out of scope. Four names are banned outright; the fifth is also a language the
editor edits through tree-sitter, so it is allowed as an identifier anywhere and as prose only in
the editor's language-support files, plus one paper title in `docs/types.md`. Two lines that read
as prose were respelled as identifiers (a `known-issues.md` row, a `package-ci.yml` comment).
Sabotaged: a banned word and a prose use in README each fail it, and an identifier use passes.

2026-10-27 (a language review: three findings scoped, the cheap halves landed): a review of
Brood as a language, apart from the runtime, wrote three docs, linked from the top of
`handoff.md`'s open threads: [`failure-encoding-cleanup.md`](failure-encoding-cleanup.md),
[`record-kind.md`](record-kind.md), [`empty-list-truthiness.md`](empty-list-truthiness.md).
Landed the same day:
- **Record leaks:** `(into {} r)` returned `r` itself, and `(merge {:z 1} r)` came out as an `r`.
  Both were confirmed on the pre-fix binary. `into` now reads a record SOURCE through `seq`
  (its fields), never `%map-pairs`. `merge` is seeded with its first map rather than `{}`, so
  `(merge r m)` keeps `r` a record, as the docs promise.
- **The last public tuple failures:** `supervisor/start-child`/`restart-child` reply a
  `failure` naming the child and the cause (it used to be the bare `[:error "supervisor:
  :start failed"]`, with the cause only on stderr), and `stream/next`'s error arm is a `failure`.
  `[:ok v]` stays, because a value may be `nil` or `:done`.
- **Docs:** ADR-163's tagged-vector clause is marked superseded by ADR-310.
  `brood-for-claude.md` (embedded in every `nest new`) stopped teaching it. ADR-310 clause 6
  records its 2026-09-01 reversal, and `types.md` gained a `failure` entry.

Guard: new cases in `tests/record_test.blsp` (including one across processes),
`tests/supervisor_test.blsp` and `tests/stream_test.blsp`. Each record assertion is an
expression observed wrong on the pre-fix binary, which is the inverse sabotage. Run here: `nest format --check`, `nest check` and
`--strict` (zero warnings), `banned-names.sh`. The suites run in CI. Still open: `assoc` of
`:__id__` forges an identity; refusing it costs the hottest path, so it waits for the
`Record` kind (record-kind.md §4). A payload on `failure`, and the `()`/`nil` decision, are
open too.

2026-10-27 (the short-arm admission is reverted: it lets a small helper block a worker): CI on
`7169e806` failed two tests the change reached. `lazy_load_wait_parks`: `%lazy-call`, a short
std helper the new rule admitted, ran natively while waiting for another process's module load,
and six processes blocked their OS worker (`dirty-receive-block`) before the suspend latch
caught the arm (`suspend-latched`) — the latch heals an arm only after it has blocked once.
`process_floor`: a parked process cost 2 927 B against the 2 800 B floor, the same class (short
helpers on a parked process's path now native). A size rule cannot see that a short arm waits
through a callee, which is what the old gate fenced off by accident (compute-frontier §7.1:
"the latch is LATENT on the shipped tree"). Reverted; the measurement stands — `regex` −6% is
there for an admission rule that knows which arms can reach a receive.
2026-10-27 (ADR-480, a contract check costs what changed): `type-matches?` remembers verdicts
about compound values by the identity of type and value (`core/heap/type_verdicts.rs`, per
heap; pruned by minor/major collections and `reset_local_to`, cleared by a stamp of the
global version, declared sigs, RUNTIME relocation/free and the resolving namespace), and the
kernel decides tags, `?A`, arrows, `or`s and one level of record/vector over decided parts
without the walk. The shim templates ask `%contract-args-known?` with their arguments and
`%type-verdict` on their result before any Brood runs; the record branch no longer
`partition`s; `%type-names-record?` replaces a per-check scan of `*record-ids*`. bedit's
typed character armed 7.7 → 1.8 ms (1.3 unarmed). Guard: three `gc::tests` verdict tests,
`tests/contract_memo_test.blsp`; run here with `contract`, `ability`, `typed_params`,
`sig_adoption`, `doc_examples`, `doctest`, `audit`, `std_check` and eight bedit test files,
plus `type_grammar_agreement`, `contract_error_position`, `process_wide_statics`; the suites
run in CI.
2026-10-27 (ADR-481, a grammar declared by path, loaded at its first use):
`%tree-sitter-declare-grammar path lang` records the library and opens nothing; the first
parse, query or tree naming the language loads it, once (the table's lock is held across the
dlopen). A re-declaration is a reload at the next use; a failed load is kept as a failure
(`%tree-sitter-grammar-status`) and not retried, and a language that still has a grammar keeps
parsing with it. The text-keyed parse and `tree-reparse` now resolve the grammar before the
cache and the epoch. `std/editor/treesit` wraps both (`declare-grammar`, `grammar-status`).
Guard: `declared_grammar_tests`, `tests/tree_sitter_test.blsp`, run here; the suites run in CI.
2026-10-27 (ADR-483, a lean line cache; treesit queries over a rope): two costs bedit found
adopting ADR-471. The line cache kept every line's text and copied its vector around each
edit. It now keeps no text and holds its entries in a treap whose keys are relative to their
parents', so an edit re-bases one subtree. Spans are kept only near the last three windows,
with a state checkpoint every 32 lines elsewhere. After an edit, the old lines past it are
one tentative chain that the next walk confirms at the first agreeing state. Separately,
`editor/treesit/indent-column` and the `point-*` motions take a rope, so RET stops building
the whole buffer's string. Run here: `tests/highlight_test.blsp` (seven new cases, counting
lexer calls), `tests/treesit_module_test.blsp` (rope against string, with and without a
tree), `lexer`, `doc_examples`, `audit`, and `nest check --strict` on both files.

2026-10-27 (main's red, attributed and cleared): four reds across the morning's CI runs, each
traced to its cause.
- **The strict checker gate** flagged `tests/highlight_test.blsp`'s edit script. It mixed
  `[:insert line text …]` and `[:delete from to …]` in one vector, so the checker typed the
  third slot `int | string`. Each step now carries its edit as a function (`lc-insert-at`,
  `lc-delete-lines`): one shape, and nothing was widened.
- **bedit's `testrun_test.blsp:307`** destructured a `ui-memo` marker with ADR-470's old
  arity. bedit's HEAD already reads it by index. `BEDIT_REF` moved to `e48287f5`, after bedit's
  `nest check`, `--check-boot` and that test file passed here. bedit's whole suite runs in CI:
  the smoke target is refused on this machine.
- **`receive_loop_jit_test` "parks on EVERY message"** read `:untried` in 3 of 5 runs. A full
  compile queue resets a hot arm to untried with its count at zero, and one 400-message burst
  was its only chance. It now drives through `settle`. A loop parked mid-arm never counts a
  resume, so it still fails that defect.
- **`jit_unrelated_rebind_test` "keeps rebinding"** read `rebinds` 0: the rebinder had not run
  before the arm settled, so the trickle never happened. The rebinder now signals before
  `settle` starts.

`lazy_load_wait_parks` (6 dirty-blocked waiters) and `a_parked_process_costs_no_more_than_the_floor`
failed only in the two runs that carried `c38b5bc2`, which admitted small call-mediated arms
such as the waiters' `child` to native code, and in none of the four without it. Its revert
(`f37fcdf7`) is their fix. Run here: each touched test file once (`nest test FILE`), the
strict checker over each, and `nest format --check`. The suites run in CI.
2026-10-27 (ADR-484, a release bundle starts from its own startup image): `nest release` runs
the binary it wrote with `--brood-build-image`, so the shipped runtime loads its own sources
once and writes ADR-218's sectioned image, then appends the image to the archive (an optional
`BRDIMG01` tail; the bundle format version is unchanged). At mount the runtime holds the bytes
as `mem:bundle-image`; `run-bundle` installs them against a stamp of the stdlib id, contract
mode and manifest (`system/build-id` holds the mtime, which a copy changes), and materialises
each module, falling back to source for one without a section. `project-image`'s section
writer, registration prune and installer take the carried-module predicate and the path. bedit's
bundle: first frame ~750 → ~185 ms, +5.3 MB. Run here: `release_bundle` (9, two new),
`bundle::tests`, `startup_image`, `image`, `image_section_decode`, `project`; bedit bundled,
booted and boot-checked twice.
2026-10-27 (ADR-485, `nest check` re-checks what an edit reached): bedit's strict ratchet
test spent 55 s re-checking 158 of 204 files after any edit, one at a time. Three causes. A
user global's fingerprint fact was its defining file's mtime. The reuse test ran before
`project-preload!` while the stamp was taken after it, so a lazily loaded std global read
`U` against a stamped `F`. And 4.5 MB of expensive files stayed under the pool's 7 MB
threshold. Now a closure's fact is a process-independent digest of its arms (symbols by
name, gensyms by first appearance, maps order-free); a non-function contributes its kind,
and a dynamic or earmuffed global only that it is bound. The expander records each macro
it expands. `nest check` preloads every closure before the reuse test. The pool engages at
1 MB, with chunks dealt by size. A replay prints the lines as printed, in order. bedit:
touch 53.5 → 3.3 s (1 re-checked), a one-function edit 5.5 s (3), a hub edit 7.7 s (123),
cold re-check 42 → 4.7 s. Verdicts byte-identical to the old binary's. Run here:
`types::check::tests` (574, with the new digest test), `check_preload`, `project`,
`checker_pass_agreement`, `checker_follows_live_image`, `nest check --strict` on
`project-check.blsp`, and clippy. Not run here: `check_incremental.rs` and the suites, left
to CI.

2026-10-27 (the downstream smoke's 2 GB red was a quadratic render in bedit): bedit's suite peak
went from 1.5 GB to past `nest test`'s 2 GB default soft limit. The cause was no leak: on CI's
fresh checkout, `web_fuzz_session_test` opens the root commit, whose diff is every file (~95 000
lines). bedit's `sectionbuf` rewrite built that render with a vector `conj` per line and per span,
O(lines²), past 6 GB. It was found by measuring each changed test file in both trees, then
reproducing in an identical single-commit repo. bedit `e3dde585` builds in one pass, with a
guard that hits the limit on the old builders. `BEDIT_REF` moved there.

2026-10-27 (ADR-486, context faces and verbatim indent): two gaps found adding Terraform/HCL
to bedit. In HCL every name is an `identifier`, so a face table keyed on kind alone could not
tell a block's type from an attribute's name from a called function, and Rust/Go/Python could
not colour a call's function. A face-table key may now be a selector map `{:kind :parent
:field :nth}`, matched in the native span walk from the cursor it already holds (parent kind,
`field_name`, named-child index). The most specific entry wins, weighted field > parent > nth
> kind. `%tree-sitter-spans` / `%tree-sitter-tree-spans` take the table and report its value
(the face name). `face-table-problems` validates a table. The indent spec's new `:verbatim`
names node kinds whose interior lines keep their indentation (a blank one continues the
previous line's), so TAB in a heredoc no longer rewrites its content. Run here:
`treesit_module_test` (54, with Ruby, plus HCL / Rust / Go from the installed grammars),
`tree_sitter_test`, `doc_examples`, `doctest`, `audit`, `nest check --strict` on
`std/editor/treesit.blsp`, `cargo test -p brood --lib tree_handle_tests`, and clippy
(gui, treesit-grammars).

2026-10-27 (KI-306: a second way a native arm resets, filed with the tier tests' fix): the two
intermittent tier tests were fixed upstream in `dc94e5f4` (above), attributed to a full compile
queue resetting a hot arm with its count at zero. There is a second path to the same `:untried`,
found while reproducing it: a runtime collection that moves shared code bumps `code_relocations`,
and `native_still_valid` then resets EVERY native arm in the runtime — `lonely-loop`, `inc`, `dec`
and `%reverse-acc` together, named by a temporary trace since removed. Reproduced on demand with a
second process rebinding a global to fresh closures (integer rebinds never trigger it). The arm
re-tiers after, and `settle` covers both paths. Filed as KI-306 for the record, with the counter's
coarseness noted: nothing measured asks for a per-arm check yet.

2026-10-27 (ADR-487, face-table `:text` and nested `:parent`): two cases ADR-486's selectors
could not reach. Elixir parses `def`/`if`/`case` as calls with an identifier target, the same
shape as `foo(x)`; a selector's `:text` (a string or a set of strings) tells them apart,
checked last and refused on byte length first. A method call's name (Rust `s.bar()`, Go,
JS/TS, Python, C) differs from a field read only by where its PARENT sits; `:parent` may now
be a selector map, at most three deep, matched up the context chain the walk already keeps
on its stack. Specificity is a per-level weight vector from the node outward (text 16 > field
8 > parent 4 > nth 2 > kind 1), compared lexicographically, then the narrower text set, then
the face name; the table is sorted once per call and the walk takes the first rule that holds.
`face-table-problems` validates both. Run here: `treesit_module_test` (60, Ruby/Elixir plus
installed HCL/Rust/Go), `doc_examples`, `audit`, `nest check --strict` on
`std/editor/treesit.blsp`, `cargo test -p brood --lib treesit` (treesit-grammars), `cargo
check --no-default-features`, clippy (gui, treesit-grammars).

2026-10-27 (ADR-482, fragment replay on the GPU and column damage on the CPU): the GPU target
keeps each memoised fragment's instances and batch segments (`gpu::FragmentCache`), keyed by
the fragment's `Arc` and everything its quads depend on — grid, clip, viewport, metrics, line
height, family, ground, font generation, ligatures, subpixel order, scale, a texture
generation, and the blink phase for a fragment holding a blinking caret — and pushes them whole
on a hit, carrying the runs and nested fragments it holds. The CPU painter re-rasterises a
dirty row only in the columns its differing leaves can paint (`paint::strip_columns`; `Canvas`
gained column bounds every primitive clips to) and presents rects; `BROOD_GUI_COLUMNS=0` is the
escape hatch. Guard: `gpu::reuse_tests` (replay equals expansion; every context change misses;
carry and drop), `paint::strip_diff_tests` (typing repaints its cells; every op kind matches a
full raster; a rect-confined raster is exact inside and untouched outside). Run here: the
filtered `cargo test -p brood --lib host::gui` (128) and `debug_flags`, headless; two guards
sabotaged red. Not covered: `GpuWindow::expand_fragment` against a device; nothing measured.

2026-10-27 (ADR-488, the reader-family form scans over a rope): `%scan-form-start` / `-2` /
`%scan-form-end` take a rope as well as a string, lexed in place through a chunk cursor (one
`ScanText` lexer for both). A rope's safepoint table is carried from the nearest of the last
four ropes scanned: kept before the edit, re-scanned across it, and the old tail taken from the
first safepoint whose state the new pass reproduces, so a keystroke re-scans about a form, not
the document. `text/slice` / `text/length` accept a string; `sexp/narrow`, the sexp motion
commands, `point-kill-end` and `editor/highlight/closer-redundant?` read the rope. Run here:
`cargo test -p brood --lib form_scan_tests`, `sexp_test`, `highlight_test`, `rope_test`,
`treesit_module_test`, `memo_test`, `doc_examples`, `audit`, `nest check --strict` on the
touched std, clippy (gui, treesit-grammars). Nothing timed.

2026-10-27 (the line cache told what changed): `editor/highlight/line-cache-update` takes an
optional `change` (`[start old-end new-end]`, `text/changed-range`'s shape) and uses it instead
of comparing the two ropes; `editor/buffer/buffer-splices-cover` (was the private
`buffer-cover-step` fold) folds a buffer's splices into that range, so an editor passes its
splice log's answer. Run here: `buffer_test`, `highlight_test` (told == found == a cold cache,
four edits), `doc_examples`, `audit`, `nest check --strict` on both files.

2026-10-27 (ADR-489, injections): tree-sitter could parse a document in one language only, so
code embedded in another (a Markdown fence, Elixir's `~H`, a Terraform heredoc, a Dockerfile
`RUN` line) could not be coloured as itself. Two layers on tree-sitter's included ranges. First,
`%tree-sitter-range-spans` (std `fontify-ranges`) colours given char ranges of a string or rope
as ONE document in the outer text's offsets, nil/`()` for a missing grammar; it is what a line
lexer calls. Second, `%tree-sitter-spans` / `%tree-sitter-tree-spans` / `%tree-sitter-tree-changes`
(std `fontify` / `fontify-tree` / `tree-changes`) take an optional injection spec: `{:rules
[{:content selector :lang kw | :lang-from kind :aliases {word kw}}] :faces {lang table}}`. A
parent's matching children are one document. Its spans replace the host's over its pieces, and
an interpolation between the pieces keeps the host's colours. Injected trees are parsed per call
over the window and not held, capped at 32 KB a document (cut to the window past that), 128 KB
and 64 documents a call. `tree-changes` widens an edit over the embedded document it lands in.
`injection-problems` validates a spec. The span walk now collects byte spans and converts them
once at the end. Run here: `treesit_module_test` (76; Ruby/Elixir, plus installed
HCL/Python/Rust/Go), `doc_examples`, `audit`, `nest check --strict` on `std/editor/treesit.blsp`,
`cargo test -p brood --lib treesit` (gui, treesit-grammars), and clippy (gui, treesit-grammars;
and with treesit off). Three guards were sabotaged red: one document per piece, a walk stopping
at the window, and a pooled parser keeping its ranges. Nothing was measured.

2026-10-27 (a node's anonymous children, beside ADR-489): `%tree-sitter-kids` and
`%tree-sitter-tree-kids` take an optional `all?`. With it they also answer the parent's
ANONYMOUS tokens, in source order among the named children, each `:named false` with the
token's text as its `:kind` (`:{`, `:do`). The std wrapper is `node-children` (`text lang offset
&optional all? tree`, the held tree first), which the motions' private `ts-kids-at` now calls.
bedit's tree-sitter bracket matcher was finding a pair in the middle of its parent by scanning
the text between named children. It can read the pair from this list instead. HCL wraps each
brace in a named one-character `block_start` / `block_end`, which the list returns as such.
Guards: `treesit_module_test` (Elixir `f(a, b)` with and without `all?`, held tree equals text;
the installed HCL block's children). Sabotaged red by ignoring the flag. Run here:
`treesit_module_test` (78), `doc_examples`, `audit`, `tree_sitter_test`, `nest check --strict`,
and clippy.

2026-10-27 (ADR-490, input before background): bedit froze while a language server flooded
its loop with messages, because a key and every background message shared one FIFO mailbox
and a coalescing drain folded up to 1024 of them per render. The kernel's mailbox has lanes
now: `proc/send-priority` / `proc/take-priority` (a window and a tab deliver all their input
on that lane) and `proc/send-latest` (a newer message with the key replaces the queued one).
A display gains `:poll-input`, the one definition of user input per frontend, which `ui-run`
asks before `:poll`. A coalescing turn folds background for `*ui-fold-budget-ms*` (8) and
renders, always takes input first, and makes one arrival-order fold per turn so background
cannot starve. A watchdog reports an `update` or `view` over `*ui-slow-ms*` (50) by the
input's tag, to the model's `:on-slow` or stderr. Run here: `ui_test` (77), `serve_test`,
`display_test`, `observer_test`, `gui_test`, `sse_test`, `doc_examples`, `audit`, `nest check
--strict` on `ui.blsp` and `serve.blsp`, `cargo test -p brood --lib mailbox::tests`, and
clippy (gui, treesit-grammars). Nothing was measured.

2026-10-27 (ADR-490, the gaps closed): `proc/send-latest` / `proc/send-priority` keep their
lane through a locally registered name (bedit's `:editor`) and a `{:name :node}` address
naming this node. A receive never hands over a LATEST message superseded while it was out
for a pattern check (checked for LATEST candidates only). Only USER input stamps
`:last-input`: displays gained `:poll-tagged` (`[message input?]`, over the new
`proc/receive-lane`), so an idle beat fires during a flood. A non-coalescing app takes the
oldest message on the turn after one whose input jumped the queue. `ui-coalesce-motion`
matches a window's `{:at …}` motion, which it never had. The thin client ships keys before
painting; the terminal client draws only the newest queued frame. Run here: `ui_test` (84),
`serve_test` (16), `display`, `observer`, `gui`, `sse`, `doc_examples`, `audit`, the
`receive_*` and `mailbox_order` tests, `cargo test -p brood --lib mailbox::tests`,
`cargo test -p cli --test serve_attach`, `nest check --strict` on `ui.blsp` / `serve.blsp`,
clippy (gui, treesit-grammars). The supersession and idle-timer guards were sabotaged red.
Nothing was measured.

2026-10-27 (ADR-491, quit): a fold or a view that ran for seconds froze the editor and
nothing could stop it. A process now opens an interrupt window (`proc/allow-interrupt
ceiling-ms` / `proc/inhibit-interrupt`); inside it, `proc/interrupt pid`, the window's own
ceiling, or a quit key (`proc/set-quit-key key busy-ms`, matched on the PRIORITY lane at
delivery) raises a catchable `{:kind :interrupted :code "E0047"}` at the next safepoint —
the once-per-quantum check every tier already makes, so the hot path pays nothing new. A
parked process is never interrupted. A tree-sitter parse cancels from inside through its
progress callback. `ui-run` arms `:ctrl-g` with 300 ms busy, runs `update` and `view` in a
window (an interrupted fold keeps the old model, an interrupted view the last frame), and
stops a BACKGROUND fold past 2000 ms itself; a user's command only C-g stops. Reported
through the watchdog with `:interrupted true`. Not done: the terminal and a served session
have no quit key mid-turn (their keys do not arrive on a priority lane); no busy cursor.
Run here: `cargo test -p brood --test interrupt` (all tiers, a tree-sitter parse; the root
native poll's check sabotaged red), `--lib` filters `interrupt`, `mailbox`, `quantum`,
`preempt`, the `preemption` / `reductions` / `jit` / `primitive_naming` /
`natives_never_mutate` test binaries, `ui_test` (92), `display`, `gui`, `observer`, `serve`,
the `receive_*` and `mailbox_order` tests, `doc_examples`, `audit`, `nest check --strict` on
`ui.blsp`, clippy (gui, treesit-grammars). Nothing was measured.

2026-10-27 (ADR-491, quit closed out): the interrupt is STICKY — a `catch` that binds it ends
the quantum and it is raised again until the window closes — so a catch-all inside `update`
cannot swallow C-g; `proc/interruptible ceiling-ms thunk` replaced the allow/inhibit pair,
opening, closing and catching in Rust. Long builtins poll (`eval::InterruptPoll`): `sort`
without a callback (chunked, then merged as runs), string split/join/codepoints/graphemes,
the regex scans between searches (one search stays unstoppable but linear), rope
find/rfind/skip, the plain JSON encoder, the Myers diff, `%ui-harvest`. The terminal's keys
reach the loop's priority lane through a reader thread (`term/forward-input`), and a quit key
may be a vector matched as a message head on any lane, which a served session's displays
declare (`:quit-message`, `[:key k]`). A window shows the wait cursor while a turn is past
busy-ms. Watchdog reports carry `:input`. Run here: `cargo test -p brood --test interrupt`
(14; the sort probe sabotaged red), `--lib` filters `interrupt`, `mailbox`, `quantum`,
`preempt`, `ui_test` (97, a real pty C-g among them), `display`, `gui`, `observer`, `serve`,
`sse`, the `receive_*` and `mailbox_order` tests, `doc_examples`, `audit`, `nest check
--strict` on `ui.blsp` / `term.blsp` / `serve.blsp`, clippy (gui, treesit-grammars, all
targets). Nothing was measured.

2026-10-27 (ADR-492, the thin window client's backlog; lanes over a node link): the window
client drains its daemon's queue and paints only the newest frame. It applies every
texture, sound and window message in the order the daemon sent it relative to that frame,
ships queued keys first, and carries a skipped frame's one-shot ops (`[:sound …]`,
`[:burst …]`, listed by the new `editor/display/frame-one-shots`). The daemon sends frames
with `proc/send-latest` (plainly when they carry a one-shot). `Send` frames carry their
mailbox lane, and the receiving node queues in it; the dist protocol is v11, and a v10
peer is refused at the magic. Run here: `serve_test` (21), `display`, `gui`,
`gui_robustness`, `observer`, `ui`, `mailbox_order`, the `receive_*` files, `doc_examples`,
`audit`; `cargo test -p brood --lib dist::` and `mailbox::tests`; `cargo test -p cli --test
serve_attach` and the two new `distribution` tests; `nest check --strict` on `serve.blsp` /
`display.blsp`; clippy (gui, treesit-grammars); cargo fmt. Painting every frame was
sabotaged red. Nothing was measured.

2026-10-27 (ADR-492, v10 peers keep linking; frames conflate in a link's writer queue): the
v11 bump no longer strands older nodes. The magic is now the capability exchange: a v11
responder answers a dialer in the dialer's version, and a v11 dialer that hears v10
redials in v10. A lane send is a new `FRAME_SEND_LANE`, written only between two v11 ends,
and is the plain `Send` to a v10 peer. `BROOD_DIST_V10=1` makes a runtime speak v10 for
the mixed tests. A `send-latest` frame not yet written to a link's socket is replaced in
the writer queue by the next one to the same target and key (a slot plus a ticket; the
writer passes over replaced tickets). Run here: `cargo test -p cli --test distribution`
(44, including both mixed-version directions; without the redial, the v11-dials-v10 one
is red), `cargo test -p brood --lib dist:: mailbox::tests debug_flags`, `serve_attach`,
`serve_test`, `mailbox_order`, the `receive_*` files, `doc_examples`, `audit`; clippy (gui,
treesit-grammars); cargo fmt. Nothing was measured.

2026-10-27 (ADR-491 finished; review fixes to ADR-490/491/492): `proc/interruptible` over a
VM thunk is now an interrupt REGION on the bytecode driver's own frame stack — a handler kind
beside the inline `try`, found by its frame's index — so a `receive` in `update` or `view`
parks instead of blocking its worker; the builtin stays for the tree-walker, `apply` and
native thunks, and the JIT fences the call out. The last one-pass natives poll: the case maps
(a `Σ` string is cut only after whitespace, so the answer is the whole string's),
`string/normalize`, `%str-index-of` / `%str-last-index-of`, the four coders, `%digest`,
`%hmac`, pbkdf2; rope edits are tree edits and need nothing; a bounded-length regex is
searched in 64 KB windows (`Input::range`), an unbounded one stays one capped search. A
served session tells its thin client when its loop is busy (`proc/busy?`, a watcher kicked
per input, `[:busy on]` newest-wins, `%gui-busy`). The interrupt test file runs one test
fewer without a Ruby grammar — the `cfg`-gated parse test; nothing was lost. From the review:
a hard kill is checked before an interrupt; a burst's later keys keep their input tag; an
interrupted view is never the rollback point; slow updates are rate-limited; a `:poll`
replaced over a std display is asked (`display-polls`); the remote observer takes its link's
messages as the link's; a skipped frame's sound plays before a free sent after it; the
face-table and injection validators agree with the kernel; a guest link cannot stop a turn
with a wire quit key (`proc/set-quit-key`'s `from`, `:quit-from`); `send-latest` and
`take-priority` use an exact index and count instead of walking the backlog; a rope diff
skips shared leaves. Run here: `cargo test -p brood --test interrupt` (27, with
treesit-grammars), `--lib` filters `interrupt`, `mailbox`, `quantum`, `preempt`, `string`,
`compress`, `changed_range`, `quit_key`; `preemption`, `reductions`, `jit`,
`natives_never_mutate`, `primitive_naming`; `ui_test` (103), `serve`, `display`, `gui`,
`observer`, `treesit_module`, `regex`, `zlib`, `hash`, `crypto`, `strings`, the `receive_*`
files, `mailbox_order`, `doc_examples`, `audit`; `cargo test -p cli --test serve_attach --test
distribution` (46); clippy (gui, treesit-grammars, all targets); `nest check` over
std/tests/examples and `--strict` over std+tests, zero warnings. Nothing was measured.

2026-10-28 (ADR-495, tree handles are owned): the tree-sitter handle registry was a
runtime-wide LRU of 32. A 33rd tree anywhere silently dropped the oldest: bedit with many
tree-sitter buffers fell back to full reparses, and tests running in parallel evicted each
other's trees, which is the read-the-code explanation of the one-off `treesit_module` failure
("an edit inside an embedded document…" saw `true`, a fresh parse). Now a handle is owned by
the process that opened or reparsed it (the `OwnedTable` behind sockets, children and wasm).
It is freed by `free-tree` or by its owner's exit, before the `[:down]`, and never evicted.
`%tree-sitter-tree-give` / `editor/treesit/give-tree` hand it to another local process, so
an off-loop task gives its tree to the editor before replying (bedit's `treewarm` and the
off-loop reparse must now do this). Deterministic tests: a held tree outlives 40 opened by
another process, and its update is incremental. A task's tree dies with the task unless
given. Only the owner gives. Run here: `treesit_module` (81), `tree_sitter`, `doc_examples`,
`audit`; `cargo test -p brood --lib tree_handle_tests owned::tests`; clippy (treesit-grammars
all targets, and no default features); cargo fmt. `process_wide_statics` fails on main
already (`builtins/terminal/native.rs FORWARDER` undeclared), not from this change. Nothing
was measured.

2026-10-28 (ADR-496, injection `:exclude` and line breaks): an injection rule can now name
an `:exclude` selector. The content's descendants it selects leave the embedded document and
keep the host's colours, so a Dockerfile `RUN <<EOT`'s marker is no longer bash's `ERROR`.
The last line break in a gap between two pieces is now parsed with them (not coloured), so
heredoc lines given one by one no longer run together. Before this change the second
`echo a` / `echo b` line's `echo` was an argument. Validated in `injection-problems`. Also
fixed: `process_wide_statics`, red on main since ADR-491, now declares the terminal's
`FORWARDER`. Run here: `treesit_module` (84, the Dockerfile tests with bedit's grammars),
`tree_sitter`, `doc_examples`, `audit`; `cargo test -p brood --lib treesit` (11) and
`process_wide_statics`; clippy (treesit-grammars, all targets); cargo fmt. Nothing was
measured.

2026-10-28 (ADR-495 follow-up, `owned-trees`): `%tree-sitter-owned-trees` /
`editor/treesit/owned-trees` list the tree handles the calling process owns, so a holder can
sweep handles it lost track of. A turn that opened a tree and was then rolled back used to be
cleaned up by the 32-entry LRU, and is now held until its owner exits unless swept. bedit's
`syntaxtree` sweeps on its idle beat. Run here: `treesit_module` (85; the new test was red
before), `doc_examples`, `audit`, `cargo test --lib owned::`; clippy and fmt.
2026-10-28 (ADR-493, ADR-491's two limits): an UNBOUNDED regex pattern over a text past a
window is now STEPPED — its lazy DFA driven a byte at a time by a loop of ours
(`regex_native::find_stepped`, `hybrid`'s `find_fwd`/`find_rev` without the unrolling, the
prefix prefilter kept and confined to the window), the DFA state carried across an interrupt
poll every 64 KB; every search, the lexer's and the anchored longest-match walk's included,
goes through one `find_polled`. Left as one search: an unbounded pattern with a Unicode `\b`
over non-ASCII text (the lazy DFA quits; `meta` answers) and the groups of one match a window
long. And interrupt windows NEST: each has a depth, a request is raised for the innermost
window open, a ceiling for its own window, only that window's `proc/interruptible` answers,
and closing it consumes the interrupt — Emacs's `with-local-quit`. A region carries its
window depth instead of `owner`. Run here: `cargo test -p brood --test interrupt`,
`cargo test -p brood --lib regex_native process::interrupt`, `doc_examples`, `audit`,
`regex`, `ui_test`; clippy; cargo fmt. Nothing was measured.

2026-10-28 (ADR-493 follow-up, the two regex corners): a Pike VM of ours
(`regex_native::pike_search`, regex-automata's `pikevm::search_imp` over its public
Thompson NFA, a poll per position) now takes the searches the lazy DFA quits on (a Unicode
`\b` before non-ASCII text) and the groups of a match longer than a window; the capture
engines have no step API and cannot be resumed across windows. Checked against `meta` for
every slot over twenty-two patterns × ten texts at every start. Run here: `cargo test -p
brood --lib regex_native`, `--test interrupt`, `doc_examples`, `audit`, `regex`; clippy;
cargo fmt. Nothing was measured.
2026-10-28 (ADR-494, a `receive` in an interrupt window parks on the tree-walker and through
`apply`): ADR-491's region covered a VM thunk only. The tier-0 machine now runs
`proc/interruptible` as a frame (`KontKind::Interruptible`, beside `Try`/`Binding`), so a
`receive` in the window parks a capturing machine; the VM's region unfolds `(apply
proc/interruptible …)`, and the JIT fences a chunk that reads the builtin as a value. A thin
wrapper `(fn (c t) (proc/interruptible c t))` was a passthrough whose redirect called the
builtin nested — `compute_passthrough` now refuses that head. Under a builtin's callback the
thunk still runs nested and blocks its worker; the §7.4 handoff keeps the pool moving, and an
interrupt reaches the waiter once it runs, as it does a parked one. The ui test's terminal
C-g under `BROOD_TIER=0` found an older gap: the machine redid only `%receive` after a
suspend, so `proc/receive-lane` on a capturing machine died with an empty `runtime error:`
(a top-level `(proc/receive-lane 50)` at tier 0 did, before this change); it is parked and
redone like `%receive` now. ADR-491's region test was red under `BROOD_TIER=0` (223 dirty
blocks) and passes. New `crates/lisp/tests/interrupt_one_worker.rs` (a one-worker pool,
`proc/info` barriers). Run here, at the default tier and `BROOD_TIER=0`: `--test interrupt`,
`--test interrupt_one_worker`, the `receive_*` files, `exit_test`, `ui_test`; at the default
tier `doc_examples`, `audit`, `--lib` filters `jit_subset` and `interrupt`; clippy (gui,
treesit-grammars, all targets); cargo fmt. Each fix sabotaged red. Nothing was measured; the
full suite was not run.
2026-10-28 (docs: argument-order drift; the language design pass opens): `docs/language.md`,
`docs/brood-for-claude.md`, `docs/parametric-result-types.md` and the `seq/scan` docstring
still showed callback-first calls ADR-308 had turned around — `(reduce f init coll)`,
`(fold f init coll)`, `(reduce-while f init coll)`, `(map first m)`, `(each io/puts xs)`,
`(mapcat f xs)` and others; all now collection-first. `reduce-kv` was documented as bare but is
`map/reduce-kv` (language.md, the `map` module header, Rust comments); the transducer examples
named a bare `odd?` and an undefined `sq`; a `%map-pairs` comment said insertion order. ROADMAP
gains "Language design pass (2026-10-28)", the backlog for the next month: hygiene holes, one
caught-error shape, nested quasiquote, the bare/module rule, ADR-170's stale rows, and the rest.
Docs and comments only; nothing was run.

2026-10-28 (ADR-497: ranking — optimal alignment, paths, term operators, history, narrowing):
`std/fuzzy` aligns a fuzzy term optimally (a two-row DP over the existing per-char rules;
greedy past 16384 cells), ranks paths with `{:category :file}` (+2 per char in the file name,
`^x` anchors there, fewer segments break a tie), dispatches terms orderless-style (`!x` `=x`
`^x` `x$`) with smart case, and takes a `:boost` map (history, frecency) plus an exact-match
rule over any scorer. `fuzzy/ranker` holds the options and narrows: a query refining the last
ranks only the last one's full match set (`%fuzzy-top`'s `:subset` / `:indices`). The native
pass mirrors all of it with no per-candidate allocation. `*fuzzy-scorer*` takes `(query
options)`. Run here: `tests/fuzzy_test.blsp`, `fuzzy_native_test`, `fuzzy_ranker_test`, the new
`fuzzy_ranking_test`, `doc_examples_test`, `audit_test`; `nest check --strict` on the touched
std/tests; cargo fmt + clippy. Nothing was measured.
2026-10-28 (one doctest engine, `;=>` asserts everywhere): `nest test` and `nest doctest` ran
two engines with opposite conventions — `std/tool/doctest` executed `form → result` and only
read `form ;=> result` as an illustration, while `nest doctest`'s collector in `docs.blsp`
evaluated `;=>` and its expected side too. `std/tool/doctest` is now the only engine:
`form ;=> result` is an executed case (the result read as a value), `form ;~> result` an
illustration (read, callee and arity checked, never run), and an indented `→` is reported as
an old marker rather than skipped; a second marker or a missing result is malformed. The
`docs.blsp` collector and its tests are deleted; `nest doctest` is a front end over the engine.
A one-shot Brood converter swept 1,786 cases and 551 illustrations in std docstrings and 121
and 31 in Rust builtin docstrings; the generated `defrecord` docs were fixed by hand. The
`nest doctest` integration test passed while every example failed as an unbound symbol (it
asserted only the exit code); it now asserts the compared values. Run here: the engine over
the std image, 0 failures across 2,029 documented names; `nest check` and `--strict` over
std/tests(/examples); `cargo fmt --check`; `nest format --check`. Not run: `doctest_test`,
`doc_examples_test`, `docs_test`, `blsp_dispatch`.

2026-10-28 (docstring examples take the full namespace): decided, like Elixir's doctests, that
an example runs at the top level and is written fully qualified — `(demo/lies/two)`, never
`(two)` — so it pastes into any REPL as it stands; only public names are checked, so the
qualified name is always callable. No "evaluate inside module M" capability is added. The
slip the rule invites now explains itself: a case that raises `unbound symbol: X`, or an
illustration whose head is undefined, where `X` is bound in the documented name's module,
ends `— examples run at the top level: write `module/X``. `docs/language.md`'s one leftover
`→` example is converted. Run here: the hint over `string/trim` with a bare and a qualified
example (probe), `nest check` and `--strict`, `nest format --check`. Not run: `doctest_test`.

2026-10-28 (doctest runs markdown blocks): `nest doctest FILE.md…` runs every ```clojure,
```lisp and ```brood fenced block (`doctest/markdown-failures`); ```clojure fragment, untagged
blocks and other languages are skipped. Each block runs in a child process of its own
(`nest run --no-check`, or `brood` when that is the running binary) under a 10 s timeout: in
one process a block's `def` reaches every later block, and a block that serves or sleeps
stalls the run. Unmarked lines are setup, run in order, and setup that raises fails the block
so a broken block cannot pass by never reaching its cases; a marked line may close a form the
setup opened, so a multi-line `let` ending in ` ;=> 3` is one case. The child prints its
faults on a `doctest-faults:` line; a block with no such line (a timeout, a crash, a `halt`)
is reported with its exit status. A trial over README, `language.md`, `brood-for-claude.md`
and `pattern-matching.md`: 164 blocks in 57 s, 120 failures, mostly prose after a result and
setup naming values the block never defines (the triage is ROADMAP A3 step 5). Run here: the
std doctest engine (0 failures over 2,035 names), `nest check` and `--strict`,
`nest format --check`, and probes of isolation, `halt` and the timeout through
`nest doctest`. Not run: `doctest_test`.

2026-10-28 (`:doc-paths`): the manifest names the markdown plain `nest doctest` runs after
the docstrings — files, directories (every `.md` under them) and `!`-prefixed paths left out
(`docs/doc-files`). The default is `README.md` when there is one, so a project without a
README sees no change; a path the manifest names that does not exist is reported as a
failure, so a typo cannot check nothing. Brood's manifest names `README.md` and `docs`,
leaving out the ADRs, the devlog and the archive: 88 files. `markdown-failures` takes an
optional label so failures name the file relative to the project. Run here: `nest doctest`
in a scaffold with no README, a failing README and a missing `:doc-paths` entry; `doc-files`
over this repo; the std doctest engine (0 failures over 2,036 names); `nest check`,
`--strict`, `nest format --check`. Not run: the test suites.

2026-10-28 (doctest triage; ADR-498): the first full `nest doctest` over Brood's 88 doc files
found 184 failures in 252 blocks; it is now 0 (143 blocks run, 109 tagged `fragment`, 29 s).
Most were marker spelling (`; =>`), prose after a result, and snippets naming values defined
elsewhere. Real drift, fixed in the docs: `fmt` (now `string/interp`); `string/->number`
documented as nil-or-falsy where it returns a truthy failure (its Rust docstring and two
checker comments said the same); `unimpl`/`unregister-impl` (now `%`-prefixed); `println`,
`eprintln`, `rand-int`, `string-length`, `substring`; old-order `map`, `every?` and
`string/join`; a geometry `area` returning an int under its own `:-> float`; a node cookie
below the 16-byte minimum; `reflect/check`'s message. The run also found an engine bug: an
arrow inside a string literal, or on a multi-line string's continuation line, read as the
retired marker (the existing `doctest_test` case for it would have failed in CI). The engine
now scans outside strings and skips a block line inside an open string. ADR-498 records the
doctest design; `docs/tooling.md` gains "Checking examples: `nest doctest`". Run here: the
full `nest doctest` (0 failures), the std doctest engine (0 over 2,036 names), `nest check`,
`--strict`, `nest format --check`, `cargo fmt --check`. Not run: the test suites.

2026-10-28 (`diff/map-line`; ADR-499): `std/diff` answers where a line of an old text went in
a new one — `map-line`, and `map-lines` for many positions over one diff — exact for an
unchanged line, the same offset in the replacing block for a changed one, the following line
for a deleted one, clamped to a line a cursor can be on. `hunk-changes` turns a parsed
unified-diff hunk into zero-context hunks, so `hunks-status` (split out of `line-status`)
reads a diff git printed the way it reads two texts. bedit's time machine dropped its two
private copies; its auto-revert and format-on-save now keep point on its line. Run here:
`tests/diff_test.blsp`, `tests/doc_examples_test.blsp`, `tests/audit_test.blsp`, `nest check
--strict std/diff.blsp`. Not run: the full suites.

2026-10-28 (ADR-500, hygiene at every binding site): a template's binders were renamed only
in plain-symbol `let`/`letrec`/`fn` positions. A probe macro binding `a` and splicing the
caller's `a` captured it in ten of twelve shapes: destructuring `let`/`fn`, `match`,
`when-let`, `if-let`, `for`, a template `defn`'s params and more. The rename now follows the
one pattern grammar (pins, `or`, `bytes` sizes, `:or` defaults are references; a repeated
name keeps one replacement) and a table of the standard binding macros' shapes (pairs,
clauses, `catch`, `as->`, params). It also turned up a live bug: a typed template param
`(fn (x:int) …)` became `x:int__609`, losing the annotation and the body's binding; it is
now `x__609:int`. Still literal by decision: a user's binding macro, computed binders,
`{:keys}` names. Run here: the probe over 24 shapes, `nest check`, `--strict`, the std
doctest engine, clippy `--all-features` on the crate, `cargo fmt --check`. Not run: the
test suites (`tests/hygiene_test.blsp` gains a block per site and a cross-process case).

2026-10-28 (ADR-501, every error is a map; `catch` takes a pattern): a `catch` handler saw
four shapes, not the three ROADMAP listed — `error` and `assert` raised plain strings,
a failed pattern raised `[:match-error ctx value patterns]` (and printed as that vector
uncaught), a built-in error was a map, and `throw` was raw. Every error the language raises
is now an error map: `error` → `:error` (E0090), `assert` → `:assert` (E0091), a failed
`match`/`case`/`let`/`fn` pattern → `:match` (E0092) with `:context`/`:value`/`:patterns`
and a readable message (`match: no clause matches 3 — tried 1`). They go through one new
primitive, `%raise`, so a catch binds them with position and trace as it already did for a
contract violation. `throw` is unchanged. `catch` takes a pattern with an optional `:when`
guard; several clauses are tried in order, and an error none matches is raised again from
its first site. The checker's four readers of the old vector now share
`guards::match_failure_parts`. Run here: probes of every raise shape and catch form, the
exhaustiveness lint, `nest check`, `--strict`, the std doctest engine. Not run: the suites.

2026-10-28 (ADR-500/501 follow-up: what CI found): four causes behind eight suite failures.
(1) A macro template is resolved at its `defmacro`, before its patterns are lowered, and under
`ns_assume_own` the resolver qualified every name it did not see bound: `resolve_let` and
`collect_param_syms` knew only plain-symbol binders, so `(let ([a] …) a)` and `(fn (a:int) a)`
in a template came out `ns/a`, unbound. That predates ADR-500 (`(let ((a b) …) (+ a b))` failed
the same way); pattern binders now count every symbol in them and a typed binder its base
name. (2) `%match-small?` counted the quoted pattern list of the new `(%raise (%match-error
…))`, where the old `(throw [vector])` cost two; a two-clause match tipped into a failure
thunk, which hid its guard chain from the checker (a match's domain, the non-tail lint) and
cost a closure per run. The raise costs two again. A tree-walker `table_address_space` case
also went red (192 MB over 2000 exited processes, not reproducible here); the thunks are the
likely cause, so CI is the check. (3) A template `defn`'s params were renamed, so `map`'s
arglist read `(map coll__60 f)` in bedit's doc pane: a named definition's params now stay as
written (its signature) and only shadow. (4) Tests: the crash report says `kind :error` for an
`error`, and a test macro that exposed `a`/`b` to its body by destructuring now says so with
`~'a`. Run here: probes of each, `nest check`, `--strict`, clippy `--all-targets
--all-features`, the std doctest engine, a full `nest doctest`. Not run: the suites.

2026-10-28 (ADR-500/501 follow-up: the underlying fixes): three corrections to the entry above,
each replacing a workaround with the cause. (1) A template `defn`'s params are renamed again;
what was wrong was the DISPLAY. `ClosureArm::source_params` renders a parameter list as its
source spells it, showing a gensym by its prefix (`value::source_symbol`), and every reader
goes through it — `arglist`, `reflect/arglists`, the runtime arity error, the checker's arity
warning — so `map` reads `(coll f)` and a renamed param `(a)`, not `(a__60)`. (2) The match
splice budget counts a `(quote …)` as one node wherever it appears, not a `%raise` special
case: splicing a quoted datum copies a reference to one constant. (3) A match past that budget
binds its later clauses in a failure thunk, and the domain walk read the thunk's call as an
opaque call — so a large enough match lost every clause guard after the first, before
ADR-501 too; ADR-501 only moved where the budget fell. `DomainScope` now carries `let`-bound
nullary closures with their definition scope, and a call to one has its body's domain.
Guard: `a_match_too_big_to_splice_still_has_its_clauses_domain` (a 4-clause match; red with
the thunk rule disabled). The typed-binder hygiene test expanded a module's macro through a
QUOTED bare head, which has no namespace at run time (the same rule as doctest examples); it
uses a syntax-quote, which qualifies it.
2026-10-28 (ADR-502, a fold calls a closure reducer from Brood): the inventory behind ADR-494's
"resumable builtins" came to two natives. Every higher-order prelude function walks through
`fold`, `fold` over a list is Brood, and only the vector and range spans called a reducer from
Rust. They now fold only an inlined `+`/`*` and answer `:%fold-walk` for anything else, which
`fold` walks in Brood (`%vector-fold-walk`, `%range-fold-walk`), so a `receive` in a `map`,
`fold` or `reduce` callback — or a window opened there — parks on both engines. A walk the JIT
compiled blocks once, then the suspend-host latch takes it off native code, as `%fold-loop`
always did (checked: a list fold blocks exactly once too). What still blocks is kernel
dispatch under a builtin (a record's `num/add` under `+`). Also ADR-493's missing test: the
regex group pass over a 100 000-character match, handed an interrupt between the two phases
(`groups_polled` split out, a `cfg(test)` `InterruptPoll::raising`), stops at its first probe;
red with the pass given a fresh poll. Run here, default tier and `BROOD_TIER=0`: `--test
interrupt`, `--test interrupt_one_worker`; `ui_test`, `sequence_test`, `receive_loop_jit_test`
at both; at the default tier `--lib regex_native`, `--lib process::interrupt`,
`preemption_shapes` (serially, as nextest runs it), `preemption`, `regex`, `doc_examples`,
`audit`, the `receive_*` files; clippy `--all-targets --all-features`; cargo fmt; `nest format`.
Nothing was measured: a closure-reducer fold over a vector or range now runs a Brood loop
where it ran a Rust loop with a fast frame per call — `nqueens`, `pipeline`, `json`, `reduce`
want `make ab` on the benchmark machine.
2026-10-28 (dap_test's lost `exited`, and binding-macro binders in templates): the DAP session
test failed once in CI with every step passing until `continue`, after which no `exited`
came. The dap tests ran in parallel, each session loading the same `dap-fixture` module and
calling `untrace-all` then `break-fn` on `dap-fixture/square`. `break-fn` recorded the
original but never stamped the rebinding generation, so `debug-original` took its wrapper for
the original: interleaved, the second session wrapped the first's wrapper, the first program
parked twice, and `continue` released only one. `trace-fn` and `break-fn` now install through
one `debug-install`, so a name broken twice parks once (and `untrace-fn` keeps a redefinition
made while a name was broken). A DAP session now undoes only the breakpoints it set, and does
so on a closed socket as well as on `disconnect`, instead of `untrace-all` over the runtime.
The dap tests are `:serial`: a module's definitions are global to the runtime, so two
sessions on one program cannot be isolated. Guard: "breaking a name twice parks a call once"
(debug_test; with the old `break-fn` the second park is never resumed and it times out).
Separately, the resolver now reads the hygiene pass's shape table: inside a template it
qualified the binders of `defn`, `when-let`, `for`, `catch`, … under a runtime `eval`, and a
qualified name is never renamed, so they escaped ADR-500 (CI's `hygiene_test:145`).
2026-10-28 (ADR-503, quasiquote has levels): a template inside a template is no longer an
error. The expander carries a level — 1 in the template being built, one more per nested
`` ` ``, one less per `~`/`~@` — and fills only a hole that reaches 0; deeper, it rebuilds
the form as written, markers included. The passes that read templates before they expand
learned the same count. The resolver's template depth lowers by one at a hole instead of
returning to code, so an inner template's free names qualify to the defining module (a
generated macro calls `nq-maker/tag-with` from a module that defines its own `tag-with`), and
a quote inside a template now resolves its holes (`'~(helper x)` left `helper` unqualified
before this, nested or not). The hygiene pass leaves an inner template's binders for the
macro that runs it, but renames a hole back to level 1, which reads the outer binders. An
`x#` is replaced at level 1 only. A nested template takes the runtime expansion path. The
root escapes ADR-423 writes into a macro's templates are written at every level. The checker already walked
templates by level. Guard: `tests/nested_quasiquote_test.blsp`; `syntax_finalization_test`'s
ADR-152 rejection case now pins the level-2 copy instead. Not run here (no tests on the
desktop): the suite; checked here, a script of the cases in the tests run once with
`brood`, fmt, clippy `--all-features`, `nest format --check`, `nest check` and `--strict`.
2026-10-28 (v0.45.0, tagged ahead of CI): tagged on `ad737a7d` at the user's call, before
main CI reached a verdict on it — every push of the day cancelled the run before it, so the
nested-quasiquote work (ADR-503) never finished a main run before the tag. The manual nightly
(run 40727675509, on `0af2f07f`) failed GC stress and suite run 1 on one case,
`nested_quasiquote_test:62`: a macro's inner template writes a root call head as `/name`, so
the case's `(list ~x)` read `(/list 7)`. A test expectation, fixed in `ad737a7d`, the tagged
commit. Open when tagged: that nightly's ASan job (does building `brood`/`nest` beside the
harness fix the doctest and C-g tests; do the JIT tests still read `:untried`/`:bailed` under
load — `settle` now prints why), both soaks, and ADR-502's benchmark, never run:
`make ab BASE=11601fe6 ROWS="nqueens pipeline json reduce"` on the benchmark machine. If any
of these turns up a defect, it ships as v0.45.1.
2026-10-28 (ADR-504, `tcp-send` waits on its peer): outbound sockets had no flow control —
`tcp-send` queued and returned, and the reactor's 16 MiB cap dropped the connection of a
reader that fell that far behind. So a server streaming to a slow client made the whole
response at once and held it, and one past 16 MiB was cut off: the mur forge's streamed
`.tar.gz` of Brood's trunk (21 MB) would have failed for nearly every client, and its release
downloads over 16 MiB already did. Each stream now has an `OutboundFlow` (bytes handed over and
not written to the kernel); `%tcp-send` returns true at 1 MiB and `tcp/send` waits for
`[:tcp-drained sock]`, sent below 256 KiB or when the connection goes by any route (a
`SendGuard` dropped with the connection, a send finding none, the reactor's death hook). A
stall bound (60 s, `tcp/set-stall-timeout`) drops a peer whose queue makes no progress, which
is what ends a wait on a peer that stopped reading. Two tests queued a large payload to a peer
not yet reading and then went on; they send it from a writer of their own now
(`tcp_backpressure_test`'s sender, `tcp_test`'s lingering close, which closes once the writer
is parked). Guard: `tests/tcp_send_backpressure_test.blsp` (red before: the 32 MiB writer
queued it all and was dropped). Run here, each once: that file, `tcp_backpressure_test`,
`tcp_test`, `tls_test`, `http_test`, and the `primitive_docs` and `natives_never_mutate` Rust
tests; build, clippy, fmt. Not run here: the suite, GC stress, ASan and the loops the reactor's
concurrency calls for (`make green` after the push).

2026-10-28 (ADR-505, a builtin hands its Brood call back to the engine): ADR-502 left the
kernel dispatches blocking — a record's `num/add` under `+`, `compare-to` under `<`, `->seq`
under `first`, `->count` under `empty?` — each one `apply_engine` call under the builtin's Rust
frame. The builtin now returns `Control::Redirect` with the call waiting on the heap, and the
engine that called it makes the call in its place: `dispatch` loops on it (a VM closure comes
back as `Step::Tail`, a frame push), the tier-0 machine continues its dispatch loop. Only for an
engine that offered: `Heap::offer_redirect` records the argument slice it hands the builtin, and
the builtin takes the offer only when its own `args` is that slice, so a builtin another builtin
called from Rust keeps its nested call and nothing else ever sees the signal. `<` and `first`
hand back a prelude shim (`%ord-lt`, `%record-first`, …) for the step after the method. The
bytecode's `Prim1`/`Prim2` fallbacks take the call path for a map operand only (the prelude's
`count` on a record ran nested there too), and the passthrough gate refuses
`proc/interruptible` under any name (ADR-494 §3's alias). `isolate` stays nested: a frame for
it is the global snapshot, stamp and reap on every exit, on both engines. Guard:
`interrupt_one_worker`'s kernel-dispatch case, flipped from "the count rises" to flat and
widened to eight shapes; sabotaged red per case at both tiers. Run here, each once: that
binary and `interrupt` at both tiers, `doc_refs`, `sig_in_docstring`, `registry_names`,
`type_check_catalog`, sixteen record/sequence/numeric `.blsp` files at both tiers; build,
clippy, fmt. Not run here: the suite, GC stress, and a benchmark (the offer is two stores per
native call through `dispatch`; `make ab` on the benchmark machine).
2026-10-28 (v0.46.0, tagged ahead of CI again): cut at the user's call while CI on `dd009963`
(the dependency upgrade merged with ADR-505) was still queued behind the manual nightly. What
it rests on: `cargo +1.97 check --workspace --all-targets` and clippy `--all-targets
--all-features` on 1.99.1 clean, fmt, `nest format`, `nest check` and `--strict`; ADR-505's
own runs (interrupt tests at both tiers, 16 record/sequence/numeric files, bedit's full suite
on a release build of the branch). Unrun at the tag: the suite on the merged tree, the wasm
tests under wasmtime 50's new fuel charging, the nightly's ASan job and soaks, and the
benchmark rows ADR-502 and ADR-505 name. A defect any of these finds ships as v0.46.1.
2026-10-28 (ADR-506, the bare namespace is the language; ROADMAP B4): a name is bare only when
it is syntax, a function used in 15 or more of the 37 Brood projects counted (or the partner of
one), a built-in kind's predicate or coercion, or an operator; 113 moved to their family's
module in nine waves, 189 stay. Boot was the reason the old split blurred, and it was never
one: the prelude defines `seq/mapv` itself. Each wave renamed the definition, recorded the
ledger entry, and rewrote callers at the positions the checker reports unbound, so a local of
the same name is never touched; a call-head pass covered the files the checker gives up on,
skipping a file that defines the name itself (`string/repeat`) and the first pair of a binding
list (`(let (doc …))`). What the checker cannot see came out of CI and the gates: docstring
examples (run as tests), the bare-name ledger's group counts, `'/mapv` built by `defmulti`,
Brood embedded in Rust tests, the doc catalog's kernel table, and the purity lint, which reads
any `reflect/` call as an effect and so sent `type-matches?` and `record-id` back to bare. The
combinators were the one judgement the user changed: `identity`, `partial` and `comp` stay bare,
the rest are `combinator/`. Checked here: fmt, clippy `--all-features`, `nest format`, `nest
check` and `--strict`, and the std per-file checker gate over every std file; not run here: the
suite. Downstream projects move with `nest check --fix-renames`.

2026-10-29 (a match's failure thunk is walked once): zubr-relay's `nest check` never finished
on brood 0.47.0. The file was `src/relay.blsp`, and in it `handle`: one `match` over the
twenty tagged tuples `route` answers. The checker spent the time in the parameter-domain walk,
which since 11601fe6 (v0.45.0) follows a `match`'s failure thunk into its body, so that a match
too big to splice keeps its clause guards. Every clause fails to its successor's thunk from
several points (not a vector, the wrong count, the wrong tag), and that body does the same to
the next one, so walking the body at every call cost (failure points)^(clauses). A synthetic
match over N tagged tuples took 0.07 s at 4 clauses, 2 s at 12, 20 s at 16 and never ended at
20. Each thunk's domain is now walked once and kept on its binding. That is exact, not an
approximation: the body is walked in the scope it was defined in, against the same parameters,
so every call site gets the same answer. Forty clauses check in 2.4 s and eighty in 7.3 s (debug
build), and the relay file in 5 s. The guard is `a_match_with_many_clauses_is_walked_once_per_clause`
(forty clauses, and the domain must still come out): with a fresh cache per call it ran past a
120 s timeout, and with the fix it passes in 1.4 s. Checked here: fmt, clippy `--all-features`,
and that one test; not run here: the suite.

2026-10-29 (ADR-507, the callback is last and only its own arguments follow it; ROADMAP B5):
the design pass listed three suspected exceptions to callback-last. An audit of every public
`defn` and `defmacro` in `std/` with a function-named parameter that was not last (typed
parameters normalised, which the first scan missed) found seven shapes to reshape and three
classes to keep. The rule is one sentence: the subject first, a callback last, and only the
callback's own arguments after it; a call that transforms or runs a function has that function
as its subject. Reshaped: `(map/merge-with maps f)`, `(condp expr pred …)` (now subject-first
like `case` and `match`, so each clause reads in the order its test runs and `->` threads into
it), `(seq/transduce coll xform init rf)` (`reduce`'s order), `(seq/iterate-times x n f)`,
`(seq/times x n f)`, `(stream/fn-> init f)`, and the `buffer-edit` family, whose optional
`origin` sat after the transform; it is now a second arity before it, as `reduce` has two. Kept:
`update`/`update-in`/`proc/offload`/`partial`, whose trailing arguments are the callback's, and
the functions whose subject is a function. Transducer `comp` needed no change: there is one
`comp`, right to left, and items flow in written order because a stage wraps a reducing
function; language.md said "the reverse of ordinary function composition", which is what made
it look like an exception, and now says why. No checker table read any of these positions.
Callers moved with a balanced-form rewriter (57 `transduce` sites, bedit's 6 tagged
`buffer-edit` calls), each diff read by eye. Two bugs surfaced. `iterate-times` stepped once past
its last state, so `(iterate-times x 0 f)` called `f`. And a documented multi-clause `defn` with
a typed parameter bound a variable literally named `x:int`, because ADR-420's refusal read
`(fn "doc" clause…)` as single-clause. Both fixed and guarded; the iterate-times guard went red
when the fix was removed. The one stale call that could be silent, `(iterate-times inc 0 4)` read
as `(x n f)`, is flagged by the checker through `f:fn` and raises under contracts. The stale
"Brood has no `condp`" hint is gone from the evaluator. Checked here: fmt, clippy
`--all-features`, `nest format`, `nest check` and `--strict` over std, tests and examples, and
the nine affected test files, armed and unarmed; not run here: the suite.

2026-10-30 (ADR-508, edge bleed and filtered LCD text): compared against Emacs on the same panel
with the same font and size, a GUI window had two real differences. The margin around the grid
(inset plus the sub-cell remainder, all of it on the right and bottom) was plain background, so
bedit's mode line stopped about 8 px short of the frame. And subpixel text was swash's
`Format::Subpixel`, three whole-pixel renders shifted ±0.3 px with no LCD filter, read off zeno's
`mask.rs`, which left hard orange and blue fringes. Now an opaque, square background at a grid
edge gains `FRect`s over the margin (`backend/bleed.rs`, a frame rewrite cached per frame and
geometry, so both painters and their damage tracking need no change); bedit draws its mode
line and gutter as memoised fragments, so a fragment is bled inside and its result kept by
address, without which the first version bled nothing in the real editor, and glyphs are rasterised
at 3× width and filtered with FreeType's default weights (`lcd_filter`). The ink test went red
when the 3× transform was removed. Checked here: fmt, clippy `--all-features`, the bleed and
text-AA unit tests; not run here: the suite.

2026-10-30 (ADR-509, KI-307: a collection is due on bytes too): KI-307 said a process with a
large live heap stalled the processes beside it. The desktop runs no timing, so a new
on-demand `Diagnose` workflow ran the repro on a CI runner, splitting each slow sample into
CPU, collection and waiting. The slow foreground was not waiting and not collecting: it burned
2.5× the CPU. The memory figures then showed why. The background "with a large live heap" was
building a 60 000-element vector by `conj` in a loop, and a vector `conj` is a flat copy, so
each round made 43 GB of dead element arrays in two objects per step. The collector counted
objects only, so it barely ran: 12.4 GB resident in three seconds, then an abort on the run's
16 GB address-space cap, under mimalloc and the system allocator alike. The neighbour was
slow because the machine's memory was being churned at gigabytes a second, and nothing in the
runtime was shared. A minor collection is now also due on fresh buffer bytes (spilled vectors,
string text, unshared byte blobs) against a process-count-aware budget, and a major on the old
generation's buffer bytes. Two guards, each red with its half removed: the
`gc_byte_trigger_test.blsp` cases (peak 289 MB without the fix, 60 MB with it) and a collector
test that keeps 144 MB of dead vectors without the major's half. Found on the way and not
fixed: a preempted process wakes no idle worker, and an idle worker can sleep up to 500 ms
(the repro's foreground is the root thread, so it cannot show whether that matters). Checked
here: fmt, clippy `--all-features`, the new test file and the collector test, with the
sabotage runs; not run here: the suite and the benchmarks.

2026-10-30 (ADR-510, a Lisp coloured a line at a time): bedit showed a stretch of
`rp2350.blsp` all in the string colour after a scroll. The windowed lex's 200-line lead-in
began inside a multi-line docstring, and `safe-restart` scans from its input's first
character as if that were outside any string, so every quote after the docstring's end paired
the wrong way. Comparing a windowed lex with a whole-file lex for every viewport top found the
two inverted windows. Now `editor/highlight/lisp-line-lexer` runs `highlight-spans` a line at a
time for the line cache (ADR-471), carrying the form stack and an open string's face from line
to line; bedit's Lisp modes name it as their `:line-lexer`. Every window of `rp2350.blsp`, and
the whole of five large source files, colour exactly as a whole-file lex. The window test went
red with the carried string state ignored. Checked here: `highlight_test`, `doc_examples_test`,
and bedit's suite on the new std (its tree-sitter tests need a grammar-built nest); not run
here: brood's suite.

2026-10-30 (KI-308, KI-309, ADR-511: a no-issues pass). **KI-308**: beside each new background
in the KI-307 repro, the root program's first sample waited 63–115 ms without running (3 of
3 runs). It is a green process on a worker, the background it spawned was queued there, and
when the root's budget ran out it re-queued behind it; the enqueue notified only that busy
worker, and idle ones back off up to 500 ms. The preemption path now wakes a parked peer when
the queue holds anything else: the stall is gone in three runs, and `pingpong` and `ring`
read 5% faster in the A/B (the rest noise). **KI-309**: the native-walk case of
`interrupt_one_worker` saw `%vector-fold-walk` latched suspend-host twice. A latch is meant
to be final, and three writers could undo it: a re-lowering reset in the same exit, a copy
of the arm adopting the code the latch left published, and — the one CI kept hitting — the
background compile, which stored its pointer over whatever the slot held, a latch included.
Every write to `jit_code` now goes through a compare-and-swap from what the writer read, or a
reset that leaves a latch alone, and a structural test fails on any new plain store. Found on
the way and not a bug: a busy process keeps its worker for seconds when nothing else waits —
by design (ADR-401) — and the stall guard's comment, which called that a quantum, now says
so. **ADR-511**: a `conj` onto a vector appends in place into the spare capacity of a buffer
it shares with the vectors built from it, past every existing vector's length, so a `conj`
loop is linear (200 000 in a fraction of a second; the copy each step would have moved
480 GB); every other write is copy-on-write and the buffer never leaves its heap. The
byte-trigger test's vector cases had used a `conj` loop to make garbage, so they now build
and drop large vectors from a range, sized so the object count cannot trigger a collection
(each case checked red with the byte trigger removed). Checked here: fmt, clippy
`--all-features`, the new and touched test files, the JIT, latch and scheduler binaries test
by test, and the sabotage runs; not run here: the suite.

2026-10-30 (ADR-510 addendum, rainbow brackets): bedit wanted Emacs's rainbow-delimiters. The
Lisp line lexer already carries the form stack from line to line, so a bracket's depth is
known exactly at any scroll position; `lisp-line-lexer` now takes `{:rainbow true}` and paints
each bracket in `:syntax/delimiter-N` by depth. The faces are registered empty, so the REPL and
the observer look as before. The rainbow test went red with the spans dropped. Checked here:
`highlight_test`, `doc_examples_test`; not run here: the suite.

2026-10-30 (ADR-513, coloured and wavy underlines; review fixes to ADR-508 / ADR-510): bedit's
spell checker had to colour a misspelled word red because a face's underline was only a flag.
`:underline` now takes a colour or `{:color c :style :wave}`, drawn by one `underline_rects`
shared by the CPU and GPU painters, and in a terminal as the extended (curly, coloured)
underline. A review of the day's Brood work found three more things, now fixed and guarded:
`hl-string-close` read past the end of a line ending in a lone backslash inside a string
(`"a\` while typing an escape), which the line lexer reaches on every such line; the line
lexer's carried form stack kept counting forms, so an edit inside a long body changed the
start state of every later line and defeated the line cache (a frame past its third form is
now one canonical value); and the edge bleed clamped a scroll region's gliding row to the grid,
leaving it no side piece, and bled a zero-sized rect. Each guard went red with its fix removed.
Checked here: `highlight_test`, `doc_examples_test`, the `host::gui` bleed and underline unit
tests; not run here: the suite.

2026-10-30 (ADR-512, ROADMAP B6: every parameter list is a list). Of B6's four "exceptions" to
lists for code, vectors for data, two were parameter lists — ability op specs and `impl`
methods, plus `defbehaviour` ops, which the roadmap had missed — and two were data:
`defmethod`'s dispatch tuple (its parameters were already a list, so the two dispatch forms
disagreed) and `{:keys [a b]}`. The parameter lists take a list now and a vector is refused
with the list spelling as its hint, as ADR-149 did for `defn`. `defability` finds op specs by
position, since a vector no longer marks them. A Perl rewriter changed only the parameter
brackets of direct children of `(impl …)`, `(defability …)` and `(defbehaviour …)`, in code,
docstrings, comments, Markdown and Rust string literals: about 550 lists. Its first version
missed every form with a docstring (a region ended at the docstring's quote, so the form
never closed); a dry second pass after the fix found nothing. By hand: the `:derive-record`
recipes that build `(columns [r] …)`, two generators building specs with `['self]`, the
quoted specs tests feed `%register-ability`, the LSP's method skeleton, and three readers —
`introspect`'s op arity and the checker's `spec_param_types`, which took only a vector and
would have stopped checking typed op parameters silently (`ability_test` caught it). Checked
here: fmt, clippy, `nest format`, `nest check` over std and tests (the one `--strict` warning
is in uncommitted `editor/highlight` work), the changed test files, the checker, LSP and
touched integration tests, and the doc examples. Not run here: the suite;
`robustness_limits_test` aborts with a stack overflow on this machine at the last pushed
commit too, so it is not this change — investigated next.

2026-10-30 (KI-310): `robustness_limits_test` aborted with a stack overflow on this machine at
the last pushed commit too, so B6 was not the cause. Split by `describe`, then by surface: a
`fn` or `let` over a 100 000-level pattern overflowed in `typed::mentions_types`, which recursed
per level before the pattern compiler's depth cap could refuse the pattern. It walks with a
work list now; the file's 53 tests pass, and a unit test strips the same pattern on a 256 KB
thread (the recursive walk aborts it). CI's threads had enough stack, which is why it passed there.

2026-10-30 (KI-312: the parallel cold build hung on a cycle of many modules and on a broken
dependency). mur-forge's `nest check` hung after the ADR-512 migration (its `_deps/hatch` still
on the old vectors) where the sequential walk reported the error at once, and bedit's cold build
on 0.49.0 hung too. A 24-module `(:use …)` ring reproduced it on every run: the one process that
breaks a cross-process cycle shortens it by one member a round, and the waiters far from the
break reach the 30 s backstop and force-load mid-cycle. Letting every member but one break at
once just moved the claims around, because each member checks on its own tick. Pool workers now
bind `*require-cycle-abandons*` and give a cycle up without retrying, which the driver already
leaves to the sequential pass. Two more pieces: the driver kills its workers at the first real
failure rather than waiting for the jobs in flight, which reloaded the broken dependency over
and over, and a waiter on a failed load raises the loader's error rather than loading the
module itself. bedit's cold build: hung to 2.4–7 s; mur-forge's 8.7 s to 2.1–3.3 s (debug). Guards
in `parallel_load.rs` and `require_failure_test.blsp`, each sabotage-verified (ADR-426 amendment).

2026-10-28 (ADR-514, keymap prefixes compose across layers): a layer binding one chord under
a prefix took the whole prefix, so bedit's terminal mode left `C-x b` / `C-x o` / `C-x k`
undefined and any mode with a `C-c` chord hid the global `C-c` map. Layers are now overlaid
(`editor/keymap/keymap-overlay`), merging prefix maps at every level; dispatch merges only the
pressed key's maps, and `scope-bindings` shadows by the same rule. Guards: layers_test
"prefixes compose across layers" and the shadow tests, keymap_test "keymap-overlay". Checked
here: `nest format --check`, `nest check` and `--strict`. Owed on the test machine: the suite,
and the sabotage (restore the first-layer-wins lookup, see "C-x b" go red, put it back).
