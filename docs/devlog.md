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
  [`2026-10`](devlog/2026-10.md), [`2026-11`](devlog/2026-11.md) — oldest first, each opening with a list of its entries. They
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

Sessions to 2026-09-23, oldest first. From there on, the entry list that opens each month's
file in `devlog/` is the timeline; the latest days are in "Recent" below.

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
- **2026-05-28** (cont.) — Demo-friendliness: stdlib + docs gaps from `archive/claude-demo-findings.md`
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

2026-11-05 (KI-313 fixed). Reproduced on CI at last, and the trigger was the cache, not the load:
once a full suite run has left its stdlib image in `~/.cache/brood`, the native-walk case alone
fails in 86% of runs; on a cache the suite never touched it passes every time, which is every
isolated loop anyone ran. The bail trace named two arms of `%vector-fold-walk`: the waiter
latched its copy, and a second copy the root had compiled, which had adopted the walk's native
code before the latch, blocked `late`'s worker. The latch is now the function's: each copy is
admitted before it can go native (adopting, or queueing its compile), and a latch takes every
admitted copy and refuses later ones (`RuntimeCode::suspend_latches`). The test also waits for
`late` before spawning `woken`, since a latch lands on exit and the two walks overlapped (48 of
15 548 failures). Guard `a_suspend_latch_reaches_every_copy_of_the_function`, red on CI with
the sweep removed; with the fix, 0 of 172 921 primed runs failed (two Diagnose runs, the
second on the final commits with no trace and four green full suites). Checked here: fmt, clippy, the
`a_suspend_latch` unit tests, `doc_refs`, `process_wide_statics`; the loops ran on CI only.

2026-11-05 (design pass item C). The "Coming from Clojure" table in `docs/language.md` has rows
for collection-first order and the single `->` pipe (no `->>`, ADR-308), `condp`'s subject-first
order, `else` (ADR-518), truthy failures (ADR-310), lexical `let` and per-process dynamic
bindings, keywords as the only callable data (ADR-165), and `()` being `nil`. Each row is written
as Brood's own design, with its reason, not as a gap with Clojure; the table's introduction now
says Brood is its own language. Then the two rough edges the rows exposed: `condp` written in the
other order now names the swap (a literal in the predicate slot is refused when the form expands;
any other non-function raises before the first test, instead of `cannot call non-function` with a
hint about `f(x)` syntax), and `->>`/`some->>`/`cond->>` carry a hint naming `->`, `$` and `cond->`.

2026-11-06 (ADR-617). A failure carries fields: `(failure/with {:status 404} "not found: " p)`,
read back with `fields` (already the way into a record). The kernel change is one optional
argument on `%failure` and a `%failure-fields` accessor; storage, printing, equality, messages
and the wire format already took any number of fields. On top of it the last two non-failure
encodings went: the HTTP clients (`request`, `fetch`, `post`, `request-on`) answer a transport
failure with a `failure` instead of a `{:status 0 :error …}` response — `failure-encoding-
cleanup.md` said `request-on` already did; it did not — and `registry-get-json` answers the
object or a failure carrying `:status`. `nest check --strict` then found 18 test reads and one
`package.blsp` read of a response that never asked whether there was one.

2026-11-06 (ADR-618). A macro's error is placed at the call. ADR-434 said it already was; it was
at the enclosing TOP-LEVEL form, because the compile pass expands the whole form first and the
expander's error had no position. `macroexpand` now roots the call and places an unpositioned
error there, which also covers an expansion the macro built (`match` → `match*`). `(reflect/
error-at form & parts)` places it at a sub-form, through an `:at` entry `%raise` strips; the
prelude's binding/clause checks use it, and `dotimes`/`dolist` moved onto pattern clauses. A
first cut also defaulted the position inside `macroexpand_1`; sabotaging it left every test
green (the loop already covered it), so it came out.

2026-11-06 (ADR-315, finished). A fold stops on a failed accumulator whatever it folds. Only
the list walk had the guard: a vector or range fold returned the failure but ran every step
after it, side effects included. The vector and range walks carry it now, and so do the counted
loops `range_fold_rewrite` writes for a `fold` with a literal `fn` — which is why the first test
still printed `1234` for a range after the walks were fixed. Guard: `failure_mechanisms_test`
"no step runs after the failure", with a literal and a named reducer over a list, a vector and
both directions of range (each guard sabotaged red).

2026-11-06 (ADR-619). One grammar for every `fn` head, one dispatch rule. Clauses run in
order, first match wins, arity and pattern clauses alike (`select_arm_by` is a `find` now, not
ADR-047's most-specific `max_by_key`), and a clause an earlier one shadows is refused — checked
in `lower_fn` and where arity arms are built, because a `fn` from a macro expanded inside its
own defining form never meets the compile pass (the first test of that site used `reflect/eval`,
which does, and passed with the check sabotaged). Every head takes `&optional` slots (`name`,
`(name default)`, `(pattern default)`), a pattern rest and a `:when` guard; `match`'s list
patterns take `&optional` too, a default computed once as a value rather than by duplicating
the success branch. Before flipping the rule a temporary audit compared both choices for every
arity-only `fn` in std, tests, examples and bedit: none differed. Found on the way: hygiene
renamed the names inside an `&optional` default in a macro template as if they were binders
(`` `(fn (a &optional (b (+ a 10))) …) `` raised `unbound symbol: +__733`, before this change
too), and resolution left a default's module-private call unqualified. And a single clause's
`:when` had never been a guard at runtime — the keyword and the guard ran as body forms — though
the checker read it as one; it now wraps the native body, so the checker keeps the parameters
(the first cut lowered it to `match*`, and `domains::a_clause_guard_constrains_the_argument`
caught the lost domain).

2026-11-06 (ADR-620). `()` stays `nil`. The checker flags a truthiness test whose type admits a
possibly-empty list beside a vector, map, set or string — the only place the asymmetry answers
differently by kind. No finding across std, tests, examples or bedit.

2026-11-06 (docs). Every doc read against the code at `e30b1cc6`, by reading and grep; `nest
doctest` over the 75 markdown files then found one failure, an ADR-617 example naming a `path`
it never defined. Thirteen finished notes moved to `archive/` (the two MCP serves keep their
URIs), with the as-built half of `node-connect.md` now in `distribution.md`. The handoff is the current picture again, 333 lines to 108, with its state notes and
perf threads moved unedited to `archive/perf-handoff.md`. The devlog's "Recent" had grown to 4 200
lines against its own rule: 241 entries through 2026-11-04 moved to the month files, no line lost
(`2026-11.md` is new). ROADMAP's design pass is marked done and its open items checked against
the code. `primitives.md` gained 233 unlisted primitives. Found on the way: `full.yml` had not completed a run since it replaced the Nightly on 2026-11-05.

2026-11-06 (KI-371, KI-372). The docs audit's one real bug: `brood-lsp`'s project file list called
a moved name and an empty default hid it, so cross-file references answered from open buffers. A
sweep of every Brood name embedded in Rust found no other. Then CI failed that push on a
retry-passed mesh test: one node's two links to a peer, registered in opposite orders by the two
ends, each closing the one the other kept. The first fix ordered such links by nonce; the first
`full.yml` showed it evicted live links, against a test that says in so many words they must not
be, and five looped runs had hidden it behind nextest's retries. The fix now is to not
make the second dial: gossip waits while a program's dial is under way. KI-373, the other reds in
that run, was a race in `http_keepalive_test`'s own server.

2026-11-06 (ADR-630). The editor toolkit and the frontends split along their seams, reviewed by
reading calls rather than `:use` clauses. `editor/buffer` is pure: the process is
`editor/buffer-server`, the disk `editor/buffer-file`, and spans of data over text
`editor/span-properties`. A thin client loads neither the daemon nor the loop (`editor/attach`,
`editor/serve-protocol`, `editor/frontend`), `editor/lexer` no longer loads the Brood highlighter
(`editor/line-cache`, and the `:syntax/*` faces in `editor/face`), and `editor/treesit` touches no
git or compiler (`editor/grammar-install`). In Rust, `builtins/gui` leaves `terminal/native.rs`
over a shared `builtins/display`, which is one reading of a face where there were three, and
`host/treesit` and the renderer split by what each part knows. Every public name that moved is in
the ledger. Found by reading on the way: three CSI scanners disagreed, and `ansi/render` took
letters only, so `ESC[3~` ate the text behind it; all of them now ask `ansi/csi-scan`.

2026-11-07 (KI-374, KI-375, KI-376). Getting the tree back to green. The per-push CI had been red
since `crash-report` landed unformatted, which also stopped the examples job before its corpora and
stress gates; those were clean. The `editor_web_test` flake, passed by retry on `d105a615`, was a
real race: `tabs` made its table on first use, so two first users could each make one, and a fresh
runtime under load lost an entry 19 times in 300. Reading `telemetry` for the same shape found a
second bug: `reset-metrics` swapped the table out from under the handlers holding it. `full.yml`'s
ASan failure was budget, not memory: the differential had passed at 1111 s of 1200 s the run before.

2026-11-07 (KI-377..380). A review of everything since v0.50.0, by area, each finding reproduced
before it was fixed. The editor split and the Rust moves came back clean, which was the expected
risk ruled out. The new features did not: `redact` knew one spelling of a secret, and `nest move`
rewrote locals and could lose the definition it moved. Fixing the second put the pattern grammar
into `scope.rs` at last, so the LSP gains it as well; the fix's own test then found a third way to
lose the definition, through an absolute path. KI-372's fix was half of one: the hold covered gossip
arriving during a dial, not a dial already under way. `.brood_crash_dump` is bounded now too, after
reaching 183 MB in this checkout.

2026-11-08. Two playground reports, each read as a family. `(reduce strings string/->number)` said
`string/->number: expected int, got string ("1")`, which reads as if parsing "1" wanted an int: the
`expect_*` readers know the primitive and the value but not the parameter. A wrong-type error now
carries the rejected tag, and `call_native` names the one argument of that tag its registered `Sig`
rejects (`radix (argument 2) expected int, …`); a one-argument call, an ambiguous pair, or an error
raised under another name is left as it was. The unused-parameter lint fired on
`(fn ((acc x) acc))` and never on `(fn (acc x) acc)`, the same function; both spellings now go
through one check. Reading for the same shape found three more: a typed parameter was checked under
its spelling (`position:int`, never readable), a parameter read by a later `&optional` default
counted as unused, and a `fn` inside a map or set literal was never linted. Then the rest of the
binding forms: only a plain symbol in `let`/`letrec` had ever been reported, because a destructured
`let`, `match`, `receive`, `catch`, the loop forms and the `if-let` family all lower to synthetic
lets the main lint exempts. A surface pass now reads each as written, by the one pattern grammar
(pins, quotes, `or`/`and`/`bytes`/`record`, map `:or` defaults; a repeated name is a read). Its
first finds included `fold-for` accumulators the body never read. Roughly 1300 binders across brood
and bedit took the `_` prefix.
