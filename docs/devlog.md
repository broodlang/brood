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
- **2026-06-06** — whole-kernel review sweep: every Rust file reviewed (duplication / style / bugs / comments), then all findings fixed — VM `quote` divergence, LSP interning leak, timer cancellation, iterative `flush_rt_pair`, `from_node` wire excision, cross-binary dedup into `cli_support`, printer control-char escapes, ~20 comment-drift fixes
- **2026-06-07** — ADR-096 round 2 (item 6, defer-set shrink): direct `letrec` self-recursion now VM-compiles for RUNTIME-region closures — the prelude `defseq` family (`map`/`filter`/`mapcat`/`remove`/`keep`), which deferred *wholesale* before. `MakeClosure` binds the closure to its own name in its captured env (the tree-walker's late-bind); a self-call optimization (`Node::SelfCall` → `Step::SelfTail`, in-place frame reset). `(count (map inc (range n)))` **~58–60% faster** on the VM than the tree-walker. Top-level `letrec`/lambda literals defer by design (LOCAL region). (An earlier "−30…−54%" figure was a noisy read of a top-level-`letrec` bench that *defers* — the `perf-stats` harness later showed it never hit the VM; corrected 2026-06-07. Lesson: measure the path you think you are.)
- **2026-06-07** — ADR-098 small-core audit: dropped the unused `lambda`/`let*` alias spellings (no `.blsp` used them) and demoted `defmacro` from a core special form to a prelude macro over a new `(%make-macro f)` primitive (the `try`/`%try` pattern). Evaluator core 9 → **8 true special forms** (`quote if do def fn let letrec quasiquote`). `letrec` reviewed and kept (irreducible — merging into `let` would break shadow-rebinding). Surface syntax unchanged, so tooling untouched; full suite green on both engines.
- **2026-06-07** — VM profiling harness + `(def x <expr>)` runs its RHS on the VM. New `perf-stats` cargo feature (`src/perf.rs`): zero-cost-off work-attribution counters (`vm_apply`/IC hit-miss/prim inline-fallback/`self_tail`/`env_hops`/`alloc`/`tw_defer`), surfaced via `(vm-stats)` + `BROOD_PERF_STATS=1`. Plus `scripts/bench-ratio.sh` — the load-robust VM÷tree-walker ratio (the only trustworthy timing on this box). Two questions, two tools (`docs/benchmarking.md`). First profile finding: the VM is **dispatch-bound** on call-heavy code (IC 99.99% hit, prim2 96% inlined, env/alloc minor) — the bytecode-lowering gate signal. Second: a top-level `(def x <expr>)` was running `<expr>` on the tree-walker (`def` is a special form → the whole form deferred); now its RHS goes through `compile::run` (falls back to the tree-walker for anything it can't compile), so `(def a (fib 27))` runs `fib` on the VM (`vm_apply` 0 → 635k). Suite green both engines, GC-stress clean.
- **2026-06-07** — `%range-reduce` callback runs on the VM. `reduce`/`fold` over a *lazy range* drive the `%range-reduce` native, which called the reducer back via `eval::apply` (tree-walker) regardless of engine — so `(reduce <vm-eligible-fn> 0 (range n))` was pinned to the tree-walker (VM/TW ≈ 1.0). Now it routes through `compile::apply_value` when `vm_enabled` (pure tree-walker under `BROOD_VM=0`, so the escape hatch / differential TW mode stay honest): **65–67% faster** on the VM (VM/TW 0.35/0.33), measured load-robustly via the eval-grid `reduce_range` bench. Suite green both engines, GC-stress clean (allocating reducer). **Attempted + reverted:** generalising the same VM-callback routing to the *other* native higher-order sites (`apply`, `%try` thunk/handler, `binding`/`isolate` body) broke the adversarial suite — running a `try`/test-framework body on the VM where it used to tree-walk surfaced a **VM↔tree-walker divergence**: a *self-referential local closure* (a `letrec` fn that captures itself — the round-2 self-name `env_define` builds a closure whose env contains itself) is **rejected by `send` when tree-walker-built but accepted when VM-built**. Reverted to keep only the proven `%range-reduce` win; the divergence (is the VM-built self-ref closure correctly send-able, or a latent cycle bug the differential harness doesn't probe because it doesn't `send` such closures?) must be understood before native callbacks can route to the VM generally.
- **2026-06-07** — refined the above divergence + wrote a handoff (`docs/handoff-vm-callback-routing.md`); paused #1/#2 because the `let`/closure/`send` area is under active edit. Precise diagnosis (HEAD `9931e1d`): it is **`let`-self-ref**, not `letrec` (both engines agree on `letrec`: call works, `send` rejects). For a sequential-`let` self-ref closure, *calling* now works on both engines, but **`send` diverges — VM accepts, tree-walker rejects** — because the VM's `let`-self-ref closure is resolved at call time *without* being **structurally** self-referential (its captured env has no `f→self` cycle), so `closure_to_message`'s cycle walk finds nothing; the tree-walker (by-ref `let` env) and both engines' `letrec` (round-2 self-name `env_define`) *are* structural → rejected. Fix (#1): make the VM `let`-self-ref structural via the same `self_name` path `letrec` uses, so `send` rejects consistently; that unblocks #2 (native-callback VM routing). Also flagged: add a differential test that `send`s a RUNTIME-context `let`-self-ref closure (the blind spot that let this ship). Plan + the #2 code in the handoff.
- **2026-06-08** — corosensei removal §8.4 step 1: the capture/resume machinery behind `BROOD_STATE_CAPTURE` (default **off** — `main` stays on corosensei). `vm_run_bc` gains `resume: Option<Suspended>` and returns `VmOutcome::{Done,Suspended}`; a clean `receive` on an empty mailbox raises `Control::Suspend` through `%receive`, `exec_chunk` intercepts it (rewinds the suspending `Inst::Call`'s `ip`) into `ChunkExit::Suspend`, and the driver captures `(frames, cur_*, ip, entry-marks, deadline)` as a `Suspended` **without unwinding** (operand stack + frame slots survive on the heap so the resume replays straight from the `%receive` call). `scan_mailbox` no-match + green + flag → `Err(LispError::suspend)`; a suspend that surfaces in a VM run **nested under a native** re-raises (the §8.1 re-run case). Capture→resume unit test (a suspend-once native) + a green-receive signal test. Suite green at the default; differential parity green; §6 plain-release KI-1 bar re-cleared (10/10 + `BROOD_GC_STRESS`). The scheduler still drives corosensei — `run_one` dual-mode + the live-migration test are §8.4 step 2.
- **2026-06-08** — corosensei removal §8.4 **step 2**: `run_one` **dual-mode** + **live process migration** (flag-gated, default off). `Process` holds `Run::{Coro|Capture}`; under `BROOD_STATE_CAPTURE` a VM-eligible body runs in *capture mode* (worker drives `vm_run_bc` directly, no coroutine), a tree-walked body keeps a coroutine (§8.1 option a). `vm_run_bc` reifies `Preempted`/`Killed` at its loop-top safepoint (the coroutine-yield analogue); `run_one` parks a `Suspended` (mailbox waiter + timer), re-queues a `Preempted`, retires `Done`/`Killed`/error. **Live migration:** a *woken* capture process has no native stack, so `wake_enqueue` re-routes it to the least-loaded worker — it resumes on a different thread (what corosensei's KI-1b pinning forbids); preempt re-enqueue stays pinned for locality. Fixes: worker threads get a `CORO_STACK_BYTES` stack under the flag (capture bodies run on them); capture-mode `receive` deadline persisted in the mailbox (`recv_deadline`) so a re-entered receive doesn't reset `after`. `tests/live_migration.rs` (§7.6) green under GC-stress + heap-verify; §6 plain-release KI-1 bar holds **flag on and off**. Flag-on bring-up surfaced the **§8.1 native-nested-receive footgun** as the step-3 blocker: a `receive` nested in `%isolate` / a gen-server `call` (side effects — spawn, `next-ref` — *before* the receive) re-runs the native on resume → re-spawned/killed children, or a fresh non-matching `ref` each resume (livelock). Clean cases (plain spawn/receive, `%try`-nested, `after`, deep-frame migration) all pass; the fix (capture *through* native frames) is step 3. Flag stays **off**, so `main` is unchanged.
- **2026-06-08** — corosensei removal §8.4 **step 3** (partial): the native-nested-receive footgun is **resolved** the BEAM dirty-scheduler way (§7.4), not by re-running. A clean *top-level* `receive` captures + migrates; a *native-nested* `receive` (reached through `%isolate`/`%try`/a HOF callback — can't be captured through the native frame, and re-running the native repeats its side effects) instead **blocks its worker** (no capture, no re-run), falling through to the yielder-less root branch of `wait_for_message`. A `CAPTURE_TOP_LEVEL` thread-local (set per `vm_run_bc` entry, restored on exit so the *innermost* driver wins) lets the `receive` gate tell a bytecode-reachable top-level receive from a native-nested one. Result flag-on: the previously-hanging files pass (`gen` 18/18, `concurrency` 33/33, `pids`/`link`/`exit`), **1852/1859** in-language tests pass, live migration + §6 bar hold, flag-off unregressed. **Still blocking the default flip:** 6 heavy **kill/monitor-of-parked-processes-at-scale** tests time out flag-on (mass-kill 100 parked, 1000 monitored → `:down`, `observer` process-info) — plain 1000-process fan-out is identical flag-on/off (41 ms), so the hang is in the `exit`→`wake_enqueue`→`Killed`-retire→`:down` path under load, not throughput. To debug next; flag stays **off**.
- **2026-06-08** — corosensei removal §8.4 step 3 (cont'd): **kill/monitor-of-parked-processes-at-scale deadlock fixed.** Root cause: a worker parked in a dirty native-nested block (blocked inside `run_one`, never returning to its run loop) still looked schedulable to `assign_worker` (empty queue + busy = low load), so processes (e.g. 1000 monitored workers being killed, or children spawned during the parent's `mfan`) got routed onto it and stranded → all-threads-asleep deadlock. Fix (§7.4 dirty-scheduler): a worker marks itself **dirty** for the block (`dirty_block`, keyed by a new `CURRENT_WORKER` thread-local set at `worker_loop` entry); `assign_worker` excludes a dirty worker (load `MAX`), and entering the block **drains the stranded backlog** off it — only *non-fresh capture* procs (the unstealable ones) are re-routed; *fresh* stay (an idle worker steals them) and pinned *coroutines* stay (KI-1b). The mass-kill / 1000-monitored / `observer` process-info tests now pass flag-on (were 120s timeouts); `adversarial` 22/22, `concurrency` 33/33, `exit` 5/5. §6 plain-release KI-1 bar holds **flag on and off** (10/10 + `BROOD_GC_STRESS`). Also fixed a status leak: the yielder-less block left `ST_WAITING` set (it returns inline, with no `run_one` to flip it back), so a `process-info` after a native-nested receive read `:waiting` while running — now reset to `ST_RUNNING` after the wait. **Last flag-on flake before the flip:** `gen_test`'s linked-spawn describe (~25%: `%isolate` reap `:kill`s a still-alive *linked* server → `:kill` back-propagates to the isolate-runner; an async-`stop`-vs-reap race capture-mode timing exposes — Brood-side to resolve). `stream_test` is WIP. Flag stays **off**.
- **2026-06-08** — corosensei removal §8.4 step 3: gen flake **fixed** + capture mode proven **correctness-equivalent**. The `%isolate` reap now `unlink_self(child)` before `(exit child :kill)`, so cleaning up a leftover `spawn-link`ed server can't back-propagate `:killed` and kill the isolate runner (the async-`stop`-vs-reap race is moot) — gen 8/8 flag-on. Full suite both modes (good binary): **1882/1902**, the **same 20 failures in both** — all environmental (≈15 tree-sitter, needs native grammars [WIP]; ≈5 package-`:git`, needs git/network, 120s timeouts) — so **capture mode adds zero failures**. Timing 218 s (off) vs 265 s (on) ≈ **+22%** (the 6× was the now-fixed deadlock/timeouts). So every capture-specific issue (the §8.1 native-nested footgun, the kill/monitor deadlock, the gen flake) is resolved; what's left for step 3 is the **flip-the-default decision** — correctness is proven, the open question is whether to eat the ~22% now (it lands before step 4's corosensei-deletion payoff) or optimise the capture hot path first. §6 plain-release bar holds flag on+off. Flag still **off**.
- **2026-06-08** — corosensei removal §8.4 **steps 3-flip + 4 done: corosensei is gone.** Flipped the default and **deleted corosensei** in one move — the `BROOD_STATE_CAPTURE` flag, the `Run::{Coro|Capture}` split, the `corosensei` dep, all the coroutine plumbing (`Suspend`/`Yielder0`/`build_coro`/`resume_coro`/`handle_coro_outcome`), and `unsafe impl Send for Process` are removed. State capture is the **sole** scheduler engine; `Process` is now plain `Send` data and `run_one` always drives `vm_run_bc` (a body with no compiled 0-arg arm tree-walks on the worker thread, its `receive`s block — the §7.4 dirty carve-out). **Stealing generalised:** every process is heap-captured with no native stack, so `try_steal` takes **any** queued process (`pop_back`), `STEALABLE` counts all queued, and the now-vestigial `fresh` flag is dropped; `CORO_STACK_BYTES` → `WORKER_STACK_BYTES`. **Regression caught + fixed in validation:** a self-recursive pass-through `(defn hog () (hog))` spun **un-preemptibly** — ADR-069's pass-through opt flagged it as a thin wrapper redirecting `hog → hog`, and the redirect loop (`compile::dispatch` + `eval::eval`) relied on `tick()`→`preempt()` to yield, which is now a no-op (only the VM driver's loop-top `tick_capture` suspends). Both redirect loops now **break a self-cycle** (redirect resolving to the same closure by identity) and fall through to the normal call path, so it runs as a VM `SelfTail`/`Call` and preempts at the loop top (the closure's own name isn't known at `compute_passthrough` — a `defn` fn is anonymous). Validation: §6 plain-release KI-1 bar 10/10 + 5/5 `BROOD_GC_STRESS`; lib + differential (engines agree) + work-stealing + live-migration + preemption all green via nextest; full suite 553/555, the 2 failures pre-existing environmental (parser deep-nest stack flake — passes with `RUST_MIN_STACK=32M`; `dist` reconnect — fails on HEAD too, so not from this work). Suite runtime back to ~25 s (the +22% capture overhead is moot now that corosensei + its 16 MiB coroutine stacks are gone).

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

## 2026-06-06 — whole-kernel review sweep: review everything, fix everything

A full review of all ~50k lines of Rust (every file in `crates/`), fanned out
across parallel reviewers per layer plus a cross-file duplication sweep, then a
fix pass over every confirmed finding. No high-severity bugs surfaced — the
06-04 kernel-audit series clearly held — but the sweep caught real items in
every category. 535/535 tests pass after.

**Bugs / behavior fixes:**

- **eval/VM: `(quote a b)` divergence.** The VM compiled extra quote args away
  to `Const(a)`; the tree-walker rejects them. The VM now defers non-2-element
  quotes so both engines raise the same arity error. Also: a top-level `~@`
  (splice outside any sequence) is now a clear error instead of silently
  building `(list 'unquote-splicing …)`.
- **introspect/LSP: the transient-interning fix completed.** 06-04 gated
  `resolve_in_source`; `signature`, `arglist_tokens`, and `source_location`
  still interned every half-typed identifier from signature-help/completion.
  All three now gate on `intern_existing` (regression test extended).
- **process: timer entries get generation-stamped lazy cancellation.** A
  `receive`+`after` loop re-armed a fresh TIMERS entry per park with no
  disarm; superseded entries fired spurious wakeups. Each park bumps a
  per-mailbox `timer_gen`; stale entries are dropped at fire time.
- **gc: `flush_rt_pair` cdr spine is iterative** (the in-code "2a hardening
  follow-up"), so a 100k-element quoted literal promoted to RUNTIME no longer
  blows the native stack at the next `runtime-collect`. Regression test in
  `tests/gc.rs`, green under `BROOD_GC_VERIFY=1`.
- **printer: control chars round-trip.** ESC/NUL printed as raw bytes despite
  the reader accepting `\e`/`\0`; the readable printer now emits `\e`, `\0`,
  and `\u{..}` for other C0 chars (reader already supported `\u{}` — no reader
  change). `docs/language.md` escapes row updated.
- **net: unclaimed-accept reaper + a loud BINARY-UNSAFE contract.** Accepted
  sockets never claimed via `tcp-controlling-process` leaked fd+registry
  forever (DoS surface for brood-net); reaped after 30s. The
  `from_utf8_lossy` text-only limitation is now documented prominently —
  faithful binary delivery needs a bytes value kind first (**roadmap
  candidate**).
- **types: variadic-defn false-positive guard.** `(sig …)` can't express
  rest-arity, so a declared sig on a variadic `defn` produced a *false* arity
  warning (verified real in `--check` mode). Walk now tracks variadic globals
  and suppresses the sig-derived exact arity. Dead post-expansion
  `when/unless/cond/and/or` arms deleted from the recursion pass.
- **mcp: spec-correct `-32700`** on a malformed line instead of tearing down
  the session; `value_to_json` errors loudly on `:foo`/`"foo"` key collisions
  instead of last-wins; the 30s watchdog now excludes the dispatcher's own
  catalogue rebuild. **gui:** the redundant second full-framebuffer clear per
  frame is gone, and glyph-cache probes no longer allocate a `String` per
  cluster (zero-alloc `char` fast path).
- **dist: `from_node` excised from the wire** (Monitor/Demonitor/Link/Unlink/
  Exit). The reader always ignored it in favor of the authenticated peer;
  shipping a security-sensitive vestige invited someone to wire it back up.
  **Breaking** wire change (greenfield) — `PROTOCOL_MAGIC` bumped `BRD\x04` →
  **`BRD\x05`** so a cross-revision link aborts cleanly at the magic check
  instead of mis-decoding the shifted fields mid-session (caught by the
  follow-up regression review). Stale `Auth` MAC-formula doc now points at
  `compute_mac` as authoritative.

**Duplication folded (the drift-prone ones):**

- The engines' passthrough-redirect core (tick + deadline + callable gate) is
  one helper — the two copies had already drifted slightly. Runaway-guard
  error construction (stack/mem/deadline) likewise.
- The wakeup protocol (`waiter.take()` + enqueue at `exit`/`deliver`/
  `wake_for_timeout`) is a single `wake_parked`; registry-lookup-then-act is
  `with_mailbox`.
- The two binaries: `eval_file`, the main-stack bootstrap
  (`run_on_main_stack`), the terminal guards, and `read_source_or_exit` all
  live in `cli_support` now; nest's release mechanism moved to its own
  `release.rs`. The remaining `call_form` migration sites finished.
- builtins: the triplicated frame-op prologue (`term-draw`/`term-emit`/
  `gui-draw`) shares one `frame_ops` extractor; bitwise/bigint type errors go
  through `wrong_type` (offending value shown) like everything else;
  face-keyword interning hoisted out of the per-op render path.
- heap: the LOCAL accessor poison+epoch preamble is a `local_gc_check!` macro
  across 6 accessors; slab live-count/bytes sums deduped. dist: six
  `*_remote` senders route through one `send_frame` with uniform error
  handling; `MAX_DECODE_DEPTH` is defined from `MAX_MESSAGE_DEPTH`.
  lsp: span→Range is `LineIndex::range` (8 call sites); cross-file
  references/rename reuse open buffers' cached `Analysis` instead of
  re-parsing per request. syntax: one `is_trivia_ws` + `skip_line_comment`.

**Comment drift** (~20 sites): stale MAC formula, `mouse_to_value`
contradicting its own test, the interner's "one allocation" claim, misattached
doc blocks (`deregister`, `arity_message`, `cmd_doc`/`cmd_update`, slurp/
read-line + file-mtime/file-size bleed), and newly documented load-bearing
invariants — `scope_at`'s `<=` tie-break (shadowing depends on it), the
mailbox `scanned` write-before-read contract, gui's paint coordinate contract
and borrow-order notes, `EnvId::GLOBAL`'s reserved region 0b11 (now
debug_assert'd at every mint site).

Reviewed-and-accepted (no change): the message-copy vs wire-codec vs bundle
"duplication" is intentional layering over the shared `Message` seam; the
eval/VM split is disciplined (VM calls into `eval::` helpers — no drift
found); keyword tables are deliberately disjoint; printing is single-sourced.
The parked-waiter teardown leak in an embedded long-lived `Interp` is
documented, not fixed (no teardown drain path today).

## 2026-06-06 — std/ review sweep: the Brood-language counterpart

The whole-kernel sweep's counterpart for the ~12.8k lines of Brood source:
every `std/**/*.blsp` reviewed for bugs, duplication, and dead code (parallel
per-area reviewers + a cross-module duplication pass, findings re-verified by
hand before changing anything).

**Fixed:**

- `json.blsp`: the encoder didn't escape `\b` (U+0008) / `\f` (U+000C), which
  the decoder *does* decode — so a string holding either control char broke
  `parse ∘ encode = identity` and emitted invalid JSON. Added both to
  `json--enc-escapes` + a round-trip test.
- `tool/test.blsp` `collect-loop`: the central `cond` had been mangled (a body
  and the next clause's test sharing a line; one line of ~570 spaces before
  `:down)`) — the known formatter shuffle of single-`;` comments sitting
  between a `cond` test and its body. Reflowed with `;;` comments above each
  clause.
- Redundant `(require 'mod)` after a `(:use mod)` header clause removed across
  8 modules (layers, ui, lineedit, repl, sexp, package, mcp, observer) —
  `:use` auto-loads, the require was pure duplication. Informative per-dep
  comments moved onto the `:use` clauses; observer keeps `(require 'proctree)`
  (used qualified, not in its `:use` list).

**Reviewed-and-accepted (no change):** buffer/lineedit's identical
`--scan-fwd/bwd` ↔ `--fwd/bwd` helpers and `buffer--clamp` ↔ `pane--clampi`
(private one-liners; extraction would mint a module for <10 lines);
dockerfile/dotenv/markdown each carrying their own small line-walker +
`lead-ws`/`find-char` scaffold (independent grammars, shared framework would
couple them); task.blsp's repeated receive clauses across the timeout/
no-timeout branches (receive clauses can't be abstracted by a function — a
macro isn't worth it for 3 lines). The pending `range` rewrite in the
prelude diff traced correct (downward cons, no reverse).

Follow-up, same day: the remaining strictness gap closed too — the encoder
now escapes *every* raw control char < U+0020 as `\u00XX` (strict JSON
forbids them all), via a load-time `[raw escape]` table over the existing
`json--cp->string`/`json--int->hex` helpers, folded in after the named
escapes (so the introduced backslashes aren't re-escaped).

## 2026-06-06 — reducible lazy range (Value::Range)

Driven by the cross-language benchmark suite: `reduce` (fold over `(range 1M)`)
was the worst result by far — 90× off Python on wall and the *only* memory
outlier (130 MB), because `(range n)` materialised a million-cons list before
folding. Decomposition showed ~⅔ of the time was the build, not the fold.

First, a free prelude win: `range` builds ascending by **counting down** from
the top value so it cons-es the list in order with no closing `reverse` — n
allocations, not 2n (1.58 s → 0.94 s, 130 MB → 100 MB).

Then the real fix — a **reducible lazy range**. New `Value::Range(VecId)`,
backed by a `[lo hi step]` vector so it rides the Vector GC / region /
forwarding machinery; `tag()` reports it as a `Pair` so `type-of` and the type
lattice treat it exactly like the list it stands in for. `fold` / `reduce` /
`sum` / `count` fast-path through a native counted loop (`%range-reduce`, which
roots the accumulator *and* the fn handle across each `apply` safepoint) — zero
allocation. Everything else realises on demand: `seq` → list, plus `first` /
`rest` (tail is another range, no copy) / `=` (element-wise vs a list, alloc-
free) / `hash` (byte-identical to the equivalent list) / printer / cross-process
copy. Empty ranges are `nil`, so a `Range` always has ≥1 element. Considered and
rejected actual lazy *sequences*: a thunk-per-element prototype was *slower* than
materialising (closure-force + cell alloc > a cons), and improper lists only
help iolists, not folds — reducers are the right tool here.

Result: `(reduce + 0 (range 1_000_000))` 1.58 s → **0.27 s** and 130 MB →
**20 MB** (90×→12× off Python on wall, 14×→2.2× on memory) — the suite's worst
case and only memory outlier, gone. Output byte-identical to the old range
across all arities; full in-language suite green on both engines, GC-safe under
`BROOD_GC_STRESS` / `BROOD_GC_VERIFY`.

## 2026-06-06 — ADR-096: VM perf round as the JIT runway (plan)

Asked "how hard is a JIT?" and ran the analysis to ground. Conclusion: the
architecture is unusually JIT-friendly — immutability (no write barriers), the
lexically-addressed `Node` IR, the per-arm deopt seam, the `Prim2` epoch-guard
pattern, and frame-slots-on-`Heap::roots` (a tier-1 JIT keeps values in slots
across safepoints and sidesteps stack maps under the moving GC) — but the
highest-value VM-interpreter work and the JIT prerequisites are mostly the
*same list*. So: **no JIT now, no parallel track** — one road. Recorded as
ADR-096 + `docs/vm-perf-and-jit-runway.md`.

The round (benchmark between every step; archived baseline first):
call-site ICs on `Node::Call` → global-read IC on `Node::Global` → wider
inlined prims (`Prim1`, float fast path) → compile-time GC-pure bit to skip
operand rooting → `exec_value`/`exec_tail` split → (stretch) defer-set shrink.
JIT-alignment rules adopted now: one IC mechanism (the epoch-guarded slot);
never cache a resolution without a guard; indirection tables over in-place
patching (machine code can't be `rewrite_node`-patched under an ADR-091
compaction); explicit safepoint discipline; the packed-64-bit `Value` decision
flagged as open-before-1.0. Actual codegen (Cranelift) stays gated on bytecode
lowering + a real editor profile showing dispatch dominates.

## 2026-06-06 — ADR-096 round 1 shipped: ICs, wider prims, rooting skip, exec split

All five items of the VM perf round landed in one session, benchmarked between
every step (baseline archive `2026-06-06T10-45-03Z`, final `2026-06-06T12-48-07Z`).
Net on the VM: **fib −22%, sum_tail −26%, cons_build −42%, sort_brood −13…−24%,
spawn_fanout −25%** — call-paths roughly **1.2–1.7× on top of the Stage-3 VM** —
with maps/pattern/mapcat flat (no regressions). Full Rust + in-language suites
green after every item, including the `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` gate.

1. **Call-site inline caches** (the big one: −13…−31% alone). A compiled
   `Node::Call` whose callee is a free global symbol carries a `site` id into a
   per-process heap-side table (`Heap::vm_call_ics`); the entry caches the
   resolved callee + (for a non-passthrough VM closure) its `Arc<CompiledArm>` +
   captured env, validated by `(sym, argc, epoch)`. A hit skips the `env_get`
   walk, dispatch's passthrough probe, the `vm_cache` hash lookup, and the env
   read. Two GC/semantics cruxes: the epoch is *re-checked after argument
   evaluation* (an arg's `def` or a RUNTIME compaction mid-call drops the fast
   path; the rooted callee value still takes the correct generic path — old
   callee, as the tree-walker would), and **dynamic symbols are never cached**
   (`binding` rebinds without an epoch bump; `defdyn` itself bumps, so a
   pre-defdyn entry self-invalidates). Entries hold only immovable handles; the
   table clears wholesale on `runtime_collect` (sym+argc validation makes a
   recycled site id harmless). New targeted tests in `tests/basic.rs`
   (redefinition *within* a hot form via `eval`, `binding` shadowing through a
   hot site, `Prim1` guard).
2. **Global-read IC** (`Node::GlobalIc`, −1…−1.5%). Same mechanism for
   value-position global reads; call heads stay plain `Node::Global` (the call
   site's own IC subsumes them). Small, kept as the unified-IC consolidation
   (ADR-096 rule A).
3. **Wider prims**: `cons` joins `Prim2` (allocates → handled in the exec arm,
   off the numeric hot path), float/mixed-numeric fast paths in `prim_apply`
   (`(F,F)`/`(I,F)`/`(F,I)` for `+ - * < <=`, non-zero `/`; `=`/BigInt/edges
   defer), and a new `Prim1` for `first`/`rest` (`Pair`/`Nil` inline,
   vectors/ranges/errors defer). cons_build −9%, sort −7…−10%.
4. **GC-pure rooting skip** (`Prim2::broot`): operand `a` is rooted across `b`'s
   eval only when `b` can reach a safepoint; a `Const`/`Local`/`Global`/`GlobalIc`
   leaf can't, so `(+ acc n)`-shaped ops run with zero operand-stack traffic
   (the fallback dispatch still roots both). −5% on the numeric pair. `Call`
   args deliberately keep today's rooting discipline — natives may rely on
   caller-rooted argv.
5. **`exec_value`/`exec_node` split**: `Step` is a ~100-byte enum (`Tail`
   carries an inline SmallVec), and every leaf eval built one and `force`-matched
   it apart. Value positions now run through `exec_value -> LispResult` (no
   `Step`, no `force`); `exec_node` keeps only the tail-propagating shapes, with
   the combination executor factored into `exec_call`. −3…−7% across the board.

Honest notes: item 3 initially cost the pure-numeric pair ~+2% (code layout in
the bigger executor); item 4 recouped it. The archived final fib-25 median
(41.6 ms) caught mid-run interference — a controlled 30-sample re-run gives
33.97 ms (fastest in the archive run agrees: 33.8 ms).

**Environment gotcha discovered en route:** the package-manager `:git deps`
tests build fixture repos with real `git commit`/`git tag`, and this machine
signs commits via the 1Password SSH agent — with the agent locked, those 5
tests fail ("1Password: agent returned an error") and burn their 60–120 s
timeouts, which is exactly the "suite suddenly times out at 300 s" symptom that
initially looked like a VM regression. If `nest test` slows by minutes and
fails 5 package tests: unlock 1Password (or run the suite minus
`tests/package_test.blsp`).

## 2026-06-07 — ADR-096 round 2 (item 6): direct letrec self-recursion on the VM

Round 1's item 6 (shrink the defer set) picked up. An instrumented run revealed
the highest-value gap was real and hot: **`defseq` — the macro behind `map`,
`filter`, `mapcat`, `remove`, `keep`** — expands to a `defn` whose body is a
`(letrec (--loop (fn …)) (reverse (--loop …)))` where the inner `fn` captures
`--loop`, the in-progress letrec binder. `compile_captures` deferred the whole
closure (a value snapshot can't express recursive late-binding), so the five
core sequence ops ran *entirely on the tree-walker* — getting zero benefit from
round 1 (corroborated by round 1's own "mapcat flat" note).

**Fix, two layers:**
1. *Eligibility.* When a `(fn …)` is the RHS of a `letrec` binder it captures
   (direct self-recursion, tracked via `Scope::letrec_self`), `MakeClosure`
   builds the closure, then `env_define`s the binder name → the closure into its
   own captured env — exactly the late-bind the tree-walker's `letrec` does
   (env contains the closure; the closure captures the env; the same cycle, the
   same tracing-GC handling). A *sibling* unsafe capture (mutual recursion) still
   defers. `compile_closure` recovers the self-name by scanning the captured
   frame for a binding to itself (`Heap::env_frame_self_name`) — which a global
   `defn` never has, so late binding for globals is untouched.
2. *Speed (the self-call optimization).* A naïve eligibility fix **regressed**
   the `defseq` benches ~10–13%: `--loop`'s per-iteration tail call paid the full
   uncached local-closure dispatch (both ICs disengage for a local-capturing
   frame). So a tail call to the closure's own self-name compiles to
   `Node::SelfCall`, which the trampoline runs as a new `Step::SelfTail` —
   re-enter *this* arm in *this* env with the frame **reset in place**: no callee
   resolve, no `cache_key`/`vm_cache` lookup, no dispatch, no env re-root, no
   `Arc` clone, no frame teardown/rebuild. Safe because a letrec binder is an
   immutable lexical slot (no `def`/epoch concern). Gated to tail position +
   plain fixed arity (so `exec_node` — which the trampoline drives — is the only
   executor that needs it; `exec_value`/`force` `unreachable!` it).

**Result — CORRECTED 2026-06-07 (the original figures below were wrong; see the
note).** The self-call benefits **RUNTIME-region closures**: the prelude `defseq`
family. `(count (map inc (range n)))` is **~58–60% faster** on the VM than the
tree-walker (`self_tail` fires per element — verified with `perf-stats`; it
deferred *wholesale* before round 2). **Top-level `letrec`/lambda literals defer
by design** — their `fn_rest` is LOCAL-region and can't be baked into a cached
`Node` tree without a use-after-GC, so they run on the tree-walker (parity). The
self-call is for *promoted* closures, not top-level one-shots. Correctness: VM ==
tree-walker on every case (self-recursion, per-instance freshness, mutual
recursion via deferral, non-fn RHS, shadowing), suite green on both engines,
`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` clean; five regression tests in
`tests/basic.rs`. The bench is `defseq_map` in `benches/eval.rs` (the original
`letrec_loop` was replaced — see below).

**Original (wrong) claim, kept as a cautionary record:** "dispatch-bound
`letrec_loop` −30% at 100k / −54% at 1M; `defseq` pipeline at parity." Both numbers
were measurement artifacts. The `letrec_loop` bench is a *top-level* `(letrec (s
(fn …)) (s n 0))`, whose `fn` **defers** (LOCAL region) — so it never hit the VM;
"−30…−54%" was noise around parity. The `defseq` pipeline used a *top-level lambda*
mapper `(fn (x) (* x x))`, which also defers, dragging the measured result to
parity. The `perf-stats` harness (built the same day) exposed both: `self_tail` and
`vm_apply` were **zero** for `letrec_loop`. Lesson, the sharper version of the
earlier one: same-process Vm/Tw ratios are load-robust, **but only if the bench
actually exercises the path you think it does** — a deferring micro-bench reads as
parity, and noise around parity can masquerade as a ±30–50% effect. Measure the
path (`perf-stats` / `(vm-stats)` confirms it ran on the VM), then trust the ratio.
See `docs/benchmarking.md`.

## 2026-06-07 — ADR-098: shrink the core (drop `lambda`/`let*`; `defmacro` → macro)

A small-core audit of the public language surface. Three findings, three calls.

**`lambda` and `let*` aliases — removed.** `lambda` was a second spelling for
`fn`; `let*` a second spelling for `let` (Brood's `let` is *already* sequential,
so `let*` was a pure synonym implying a distinction that doesn't exist). A repo
grep confirmed **no `.blsp` source used either** — the whole codebase already
writes `fn`/`let`. Deleted the `kw::LAMBDA`/`kw::LET_STAR` constants and every
`|| symbol_is(.., LAMBDA/LET_STAR)` clause across the evaluator dispatch, the
checker (walk/hygiene/recursion/guards), `syntax/scope.rs`, the macro
resolver/expander, and the VM compiler. `(lambda …)`/`(let* …)` now error as
unbound symbols.

**`defmacro` — demoted from special form to prelude macro.** Same shape as
`try`/`catch` over `%try`: added one primitive `(%make-macro f)` (`Value::Fn` →
`Value::Macro`) and defined `defmacro` in the prelude, bootstrapped with raw
`def`/`fn` (it can't define itself):
`(def defmacro (%make-macro (fn (name & body) `(def ~name (%make-macro (fn ~@body))))))`.
Key reasons it's safe: the macroexpander expands the *head* before its structural
dispatch (`macros.rs:792`), and the loader is **form-by-form** (lib.rs — "a macro
defined by one form is visible to the next"), so the bootstrap `def` registers
`defmacro` before any later `(defmacro …)` is expanded. The *surface syntax* is
unchanged, so the checker / `scope.rs` / formatter / forward-ref pre-scan that
match `(defmacro …)` source needed no edits, and it stays in `SPECIAL_FORMS` for
highlighting. The hot-reload "macro redefined" warning moved into `def` (old
`Macro` rebound to `Macro`); `name_value` now also names `Macro` closures so a
macro keeps its name (`#<macro my-unless>`).

**`letrec` — reviewed, kept.** Irreducible: a macro can't build the
mutual-visibility scope without a Y-combinator (slower/uglier — *more* complexity),
and merging it into `let` would break shadow-rebinding `(let (x (+ x 1)) …)` (the
RHS would read a `nil` pre-binding instead of the outer `x`) and turn forward
references into silent `nil`s. A small language still needs *a* primitive for
mutual local recursion; `letrec` is it.

**Result.** Evaluator core: 9 spellings → **8 true special forms**
(`quote if do def fn let letrec quasiquote`). One stale grammar-test assertion
removed (it used `let*` to show regex-metachar escaping; `match*` still covers
it). Full suite green on both engines (`make test`); a defmacro smoke test
exercised user macros, prelude macros (`when`/`cond`/`->`), `macroexpand`, macro
naming, and the `%make-macro` type-error path.

## 2026-06-07 — ADR-099: `proc/gen` becomes a real gen_server

**What.** Closed the widest remaining OTP gap — the gen_server layer
(`std/proc/gen.blsp`) — entirely in Brood (no kernel surface, ADR-006). The
substrate and the `proc/supervisor` library were already ~Erlang/OTP; `defprocess`
was the laggard, handling only its own `[:$cast]`/`[:$call]`/`[:$stop]` envelopes.

Three gaps, all fixed:
- **`handle_info`.** New `(info PATTERN body…)` clause matches a non-envelope
  message — a monitor `[:down …]`, a link `[:EXIT …]`, a timer tick, a raw `send`
  — body → next state. Before this a server couldn't react to those *and they
  piled up unmatched in the mailbox forever* (Erlang-style selective receive keeps
  non-matches queued). A trailing **default catch-all now drops** any
  otherwise-unmatched message and keeps state (OTP's default `handle_info`), so the
  mailbox can't leak. Envelope clauses are ordered before `info` clauses so a broad
  `info` pattern can't swallow a `[:$call …]`.
- **Lifecycle.** `(init body…)` runs once at startup (sees the state param, returns
  the initial state — the place to `trap-exit`/`monitor`/arm a timer/transform the
  seed); `(terminate reason body…)` runs on a clean `(stop)`. The macro now expands
  to a `letrec` loop fn invoked once after `init`, so `init` doesn't re-run per
  message; the loop stays O(1) stack (tail self-call via the local fn).
- **Bounded calls.** `(gen-call pid payload)` delegates to
  `(gen-call-timeout pid payload 5000)` (OTP's 5 s default); both `monitor` the
  server, so a dead server raises at once and a crossed deadline raises — each
  catchable via `try`. Monitor always dropped (+ late `[:down]` flushed) on return.
- Added `spawn-server-link` (Erlang `start_link`) and `spawn-server-named`
  (registered name) beside `spawn-server`; kept to three helpers (ADR-011).

**Verification.** `tests/gen_test.blsp` grew from 9 to 18 tests (info path, no-leak
drop, init-once, terminate-on-stop, call timeout, dead-server fast-fail,
named/linked spawn). Full Brood suite **1416/1416 green** (`nest test`); existing
`defprocess` servers (incl. `std/log.blsp`, `tests/buffer_test.blsp`) unaffected —
the new clauses + catch-all are additive. No Rust/kernel changes, so the kernel
nextest pass is orthogonal. See ADR-099 and `docs/language.md` §"The `proc/gen`
server framework". Tiers 2–3 (timers, pid-returning `remote-spawn`, `terminate`
worker convention; `gen_statem`, `Registry`/`pg`, `Application`) are on the
roadmap.

## 2026-06-07 — scheduler: sticky `:kill` + busy-aware spawn placement

Two small scheduler changes after a bug-review of the process subsystem (the
park/wake handshake, lock discipline, exit/link/monitor paths all checked out —
these were the only actionable items).

**Sticky `:kill` (correctness, `mailbox.rs` `request_kill`).** `request_kill`
overwrote the pending exit reason unconditionally, so two racing `(exit pid …)`
calls — one `:kill`, one soft — could let the soft reason **downgrade** a latched
untrappable `:kill`. Since a CPU-bound process honours only `:kill` (at `preempt`)
and ignores soft signals, the target could survive a kill it shouldn't. Fix: once
a `:kill` is latched it's sticky; a soft reason can't replace it, but a fresh
`:kill` still upgrades a pending soft one. Deterministic unit test
`kill_is_sticky_against_a_racing_soft_exit` in `mailbox.rs`. (Erlang's guarantee
that `exit(pid, kill)` can't be undone.)

**Busy-aware spawn placement (enhancement, `scheduler.rs` `assign_worker`).**
Because a process is pinned to its spawn-worker for life (no migration, KI-1b),
the only load-balancing lever is the one-shot placement at spawn. It scored a
worker purely by runnable-queue length, which **ignores the process the worker is
currently running** — a worker draining one CPU-bound loop has an empty queue yet
no spare capacity, and would be picked as "idle." Added a per-worker `WORKER_BUSY`
gauge (set/cleared around `resume` in `run_one`) and folded it into the load
metric, so a busy-but-empty-queue worker is no longer mistaken for idle. Empty +
idle still scores 0 (so N spawns onto N idle cores still land one-per-core); the
change only bites when counts tie but a core is actually working. No new locks, no
migration, no race surface (one relaxed atomic per worker). See `scheduler.md`
§Placement.

Full Brood suite green (1422 tests) with both changes; the busy gauge is exercised
by the whole concurrent suite (a heuristic, so no isolated unit test).

## 2026-06-07 — fix: flaky `unbound` under load was test-isolation, not a core race

Chased a flaky `unbound symbol` that resurfaced under maximal load (full `nextest`
/ ~24 parallel suites). It looked like KI-1 returning, but isolating it (clean
worktree at HEAD reproduced it; my scheduler edits were exonerated) plus an
instrumented unbound site (`present_in_globals=false`, runtime `version` churning)
proved the global was *genuinely absent from the table* — not mis-resolved.

Root cause: `%isolate`'s wholesale `restore_globals` removes a global out from
under a process an isolated test spawned but never stopped (e.g. `concurrency_test`'s
`tco--srv` server). Pure test-harness artifact — `restore_globals` is test-only;
production never wholesale-restores globals.

Fix: `%isolate` reaps the thunk's still-running spawns (`:kill`) and waits for them
to deregister before restoring. New `scheduler::yield_now` (green-friendly
cooperative suspend) does the waiting — `std::thread::sleep` there would freeze the
isolated unit's own worker and starve a same-worker orphan (which is exactly why a
first reap attempt failed). nextest 3/3 green (was ~1/5); 24× unbound 9→0, total
failures halved. See known-issues.md KI-2 (2026-06-07) for the full write-up.

## 2026-06-07 — VM bench harness, perf-stats pass, apply-unfolding in dispatch

Three improvements to the VM benchmark/test infrastructure and one meaningful
performance win.

**Bench harness (eval.rs).** Stale `reduce_range` comment corrected (the
`%range-reduce`→VM routing already landed in `4af9d2a`; it now says so). Two new
`engine_grid!` benches added:
- `try_body` — a `(try … (catch _ …))`-wrapped tail-recursive `defn`. Ratio
  Vm/Tw ≈ 1.0, which is the correct expected result: the `try` macro wraps its
  body in a LOCAL `(fn () …)` thunk, which falls back to TW in both engines
  regardless of the `apply_engine` routing. Confirms no regression in try-heavy
  code; the routing benefit only lands when the thunk itself is a RUNTIME closure
  passed directly to `%try`.
- `apply_driven` — `(apply f …)`-driven tail recursion. Ratio was Vm/Tw ≈ 1.09
  (VM slightly slower) before the apply-unfolding work; see below for the after.

**perf-stats pass.** Ran `BROOD_PERF_STATS=1` on all three bench programs. Key
findings:
- `reduce_range 200k`: `vm_apply = 200004` — routing confirmed live.
- `try_body 10k`: `vm_apply = 0` — LOCAL thunk breaks the VM chain entirely. The
  `apply_engine` routing for `try` is correct but has no practical effect for the
  typical macro-expansion pattern.
- `apply_driven 10k`: `vm_apply = 2`, `tw_defer = 1` — only setup + first
  iteration hit the VM; all 10k `apply`-driven iterations were TW-bound.

**apply-unfolding in `dispatch` (eval/compile.rs).** `dispatch` now handles
`apply` inline via an outer `'apply: loop` that mirrors the TW's `'dispatch` loop
in `eval/mod.rs`. After each passthrough-redirect inner loop, if the callee is the
`apply` native (by name) with argc ≥ 2, it splices the trailing list and
`continue 'apply`s so passthrough runs again on the real callee. No new Rust frame
per iteration → O(1) stack preserved. `apply_builtin` stays on `eval::apply` for
the TW fallback path only; it's no longer reached from the VM on VM-eligible
callee chains.

Result: `apply_driven` Vm/Tw flips from **1.09 → 0.31** (~69% faster); `vm_apply`
goes 2 → 10,001 per 10k-iteration run. GC-stress + heap verifier clean (apply
unfolding holds `cur_callee`/`cur_argv` on the Rust stack, safe because
`seq_items` on a proper list doesn't allocate).

Five new differential corpus entries cover `try`/`binding`/`isolate` thunk routing
and apply-unfolding (tail-via-apply, basic splice, prefix+splice, nested apply,
RUNTIME callee). All 548 nextest cases + differential test pass.

## 2026-06-07 — scheduler: fresh-only work-stealing + the full-migration design (ADR-100)

**Landed: fresh-only work-stealing.** An idle worker now steals a *never-resumed*
process from a backed-up peer and runs it itself — the migration shape §3.1a
proved safe (first `resume` on the thief, no saved native stack to move).
`scheduler.rs`: a `Process.fresh` flag (cleared at first `resume` in `run_one`),
`try_steal(thief)` (rotating-start, `try_lock` per victim, pulls the first fresh
process from the victim's *back* and re-pins `worker_id`), `worker_loop` =
own-queue → `try_steal` → park-with-`STEAL_BACKOFF`(10 ms)-backstop, a relaxed
`STEALABLE` gate so a truly-idle pool re-parks on one atomic load, and a
`STOLEN`/`(steal-count)` counter for observability. Rebalances the spawn-burst
backlog of *unstarted* processes; does not move running ones.

**The bug that cost a cycle (worth remembering).** First cut held the worker's
queue `MutexGuard` across `run_one` — `if let Some(p) = lock(..).pop_front() {
run_one(p) }` keeps the temporary alive to the end of the block in edition 2021,
so the running coroutine's first preempt re-enqueued onto the same worker and
re-locked the non-reentrant mutex → the worker deadlocked (showed up as a 7-min
0.3%-CPU hang, not a crash). Fix: bind the pop to a `let` so the guard drops
before `run_one`. The test's `drain` had no `after`, which turned the lost process
into a silent hang — added a timeout so a lost process now fails loudly.

**Verified.** `tests/work_stealing.rs` (bursts until a steal is observed, checking
the deterministic total every burst — ~steals on burst 1 and climbing on a
2-worker pool): 20/20 release, 5/5 debug. KI-1 guard `tests/concurrency_race.rs`
clean 13/13 plain-release incl. `BROOD_GC_STRESS`. Full `make test` green (the lone
`clean_peer_exit_fires_nodedown_promptly` flake was load-induced under full
concurrency — passes 4/4 isolated; a distributed-nodes timing test, unrelated).

**Designed (deferred): full live-process migration — the stepping-VM endgame.**
Wrote up *why* a running process can't migrate and the one principled fix, so the
next person doesn't re-derive "just replace corosensei" and hit the §3.1a wall.
The blocker isn't corosensei — it's that a process's **call continuation** lives
on the **native Rust stack** (the tree-walker *and* today's VM, whose `exec_call →
vm_apply` still recurses natively; only the operand stack was reified and only
tail calls trampolined). The fix is to reify the **call/frame stack** as heap data
(a `Vec<Frame>` + flat dispatch loop), making a paused process plain `Send` data
`(frames, operands, ip)` — then suspension is "stop stepping," migration is "move
the data," **corosensei is removed**, and the same change independently buys
**fully precise mid-eval GC** and **anytime stealing**. Everything around the
engine is already migration-ready (`Send` heaps, migration-surviving scheduler
TLS, the INV-2 handshake the fresh-steal path now exercises). Full design +
staging + acceptance bar: `concurrency-v2.md` §7; recorded as ADR-100;
cross-linked from `memory-model.md` (the recursive-vs-stepping coupling) and
`scheduler.md`.

## 2026-06-07 — bytecode stepping engine, Stage 1 (the §7 endgame begins)

First slice of the stepping-VM endgame (ADR-100 / concurrency-v2.md §7). The goal
is to make a process's continuation relocatable heap data instead of a native Rust
stack; the operand state already lives on `Heap::roots`, so this reifies the
*control* state — a compiled arm's `Node` body is now also lowered to a flat
**bytecode `Chunk`** run by a single non-recursive loop (`exec_chunk`), over the
same operand stack.

Scope (deliberately small, to land green): only a **call-free, handle-free** body
lowers to a chunk — leaf/control/prim/let/collection nodes (`Const` atom, `Local`,
`Global`/`GlobalIc`, `If`, `Do`, `LetBind`, `Vector`, `Map`, `Prim1`, `Prim2`). Any
`Call`/`SelfCall`/`MakeClosure` (or a movable RUNTIME-handle const) makes
`compile_chunk` return `None` and the arm runs on `exec_node` exactly as before, so
the call/trampoline machinery is untouched. `CompiledArm` gains an `Option<Chunk>`;
`vm_apply_inner` runs it (with the same entry safepoint/tick) instead of walking the
`Node` tree when `bytecode_enabled()`. Each `Inst` arm mirrors its `exec_value`/
`exec_node` counterpart exactly (epoch-guarded prim inlining + fallback, the
operand-stack/GC discipline, innermost error-position tagging).

Engine is **default-OFF** behind `BROOD_BYTECODE` (truthy enables; per-thread
`set_force_bytecode` override for tests), so default behaviour is unchanged while
it's built up. Parity proven: the differential test now runs a **third** engine
(VM + bytecode) against the tree-walker over the corpus (+8 call-free-helper
entries), green incl. under `BROOD_GC_STRESS`; the full in-language suite (1434
tests) passes with `BROOD_BYTECODE=1` (every call-free arm in prelude/stdlib/tests
on the new engine); `maps_test` green under bytecode + GC stress (the operand-stack
rooting is the riskiest part). Next: `Call`/`SelfCall` (Stage 2), then the explicit
cross-arm frame stack (Stage 4 — the migration prerequisite that retires
corosensei).

## 2026-06-07 — bytecode engine Stages 2–4: calls, closures, and the explicit frame stack

Pushed the bytecode stepping engine through to the migration-critical milestone, in
three green/parity-verified commits on top of Stage 1.

- **Stage 2 — `Call`/`SelfCall`.** `exec_chunk` returns a `Step` and shares
  `vm_apply_inner`'s trampoline with `exec_node`; non-tail call delegates to
  `dispatch`, tail call/self-call reuses the frame (TCO).
- **Stage 3 — `MakeClosure`.** Capture sources emit as leaf instructions; chunks may
  now carry movable RUNTIME handles, rewritten in place by `rewrite_chunk` under
  compaction. After this `compile_chunk` handles **every** `Node` variant — so every
  VM-eligible arm has a chunk, which simplifies Stage 4.
- **Stage 4 — explicit cross-arm frame stack (`vm_run_bc`).** The big one: a chunked
  arm and its whole chain of chunked calls run on one heap `Vec<Frame>` driven by a
  single loop — a non-tail call to a chunked arm **pushes a frame** (`ChunkExit::Call`)
  instead of recursing into `vm_apply`; tail/self-tail reuse the frame; `Done` pops.
  Calls to natives / tree-walked arms run inline via `dispatch(tail=true)` (which
  returns the resolved arm un-run for a VM closure, or an executed value otherwise) —
  they're leaves to this stack. The current frame is held in registers so a tail loop
  doesn't touch the Vec; only non-tail calls push. Every frame's slots live on
  `Heap::roots` and its env on `env_roots`, so one safepoint relocates the whole stack
  in place; each frame registers its arm in `live_vm_arms` for compaction (hot reload
  intact). The native-stack byte guard becomes a `MAX_BC_FRAMES` cap.

  **Result: a process's call continuation is no longer on the native Rust stack** —
  it's relocatable heap data (`frames` + `roots` + `ip`), the prerequisite for
  migrating a running process (§7). Visible win already: deep *non-tail* recursion
  (`(nsum 20000)`) computes on bytecode where the `Node` engine overflows the stack.

  Hot reload verified unchanged (late binding + epoch-guarded ICs + per-frame
  compaction rewrite). Parity at every stage: differential test as a third engine
  (incl. `BROOD_GC_STRESS`), full in-language suite (1434) with `BROOD_BYTECODE=1`,
  and `concurrency_race`/`gc`/`runtime_collector` green with bytecode on. Still
  default-OFF behind `BROOD_BYTECODE`. Next (Stage 5): re-add the call-site IC,
  benchmark, make bytecode the default, retire the `Node`-walk — then suspension as
  data and live-process migration, at which point corosensei goes.

## 2026-06-07 — bytecode Stage 5: call-site IC + bytecode is now the default engine

Closed the perf gap and flipped the default. Two commits.

- **5a — call-site inline cache.** The bytecode `Call` carries `(site, head)` and
  caches the resolved `(arm, env)` per `(site, sym, argc, epoch)`, skipping
  `dispatch`'s passthrough probe + `compiled_arm_for` on a hit. Crucially the callee
  is still *pushed and resolved in-order* (before the args, the tree-walker's order),
  so the IC caches only the arm and stays a pure cache — a `def` bumps the epoch and
  the stale entry drops (the in-order callee then takes the generic path). This is
  why I didn't resolve-at-call-time (that would reorder head-vs-arg evaluation and
  diverge from the reference on `(f (… (def f g) …))`).
- **5b — default flip.** Benched Bc vs the `Node`-VM (medians, isolated to avoid
  load/GC contention noise — a full concurrent bench run had falsely shown
  `cons_build` slow): fib ~33% faster, sum_tail ~34%, reduce_range ~25%, defseq_map
  ~45%, cons_build ~30%, apply_driven ~15%, try_body ~par — **faster everywhere**.
  The IC flipped fib from ~18% slower (Stage 4) to ~33% faster. So
  `bytecode_enabled()` now defaults ON; `BROOD_BYTECODE=0` is the escape hatch back to
  the `Node` walker (mirroring `BROOD_VM=0`). Full `make test` (550) green at the
  default; differential + `concurrency_race` green incl. `BROOD_GC_STRESS`.

Remaining: retire the `Node`-walking executor after a release (the `Node` tree stays
as the compile *source*); then suspension-as-data + live-process migration (§7.5),
where corosensei finally goes.

## 2026-06-08 — corosensei removal §8.4 step 1: state-capture machinery (flag-gated)

First slice of the actual corosensei-removal migration (concurrency-v2.md §8,
architecture B). The continuation already lives on the heap (the bytecode frame
stack, ADR-100 Stage 4); what still pins a parked process to its worker is
corosensei freezing the *native* stack at the `receive`. This step builds the
machinery to capture/resume that continuation as plain `Send` data — behind
`BROOD_STATE_CAPTURE`, **default off**, so `main` keeps running on corosensei until
the path is proven and then corosensei is deleted (the bytecode-default playbook).

**The flow (flag on, a clean `receive` on an empty mailbox):**
- `scan_mailbox` returns `Ok(None)` (clean no-match); `receive_match`'s `Ok(None)`
  arm, for a green process under the flag, drops its scan roots and returns
  `Err(LispError::suspend(deadline))` instead of `wait_for_message` (the coroutine
  yield). The deadline-already-expired → run-`after` semantics stay ahead of it.
- That `Control::Suspend` rides the error channel up through the `%receive` native.
  `exec_chunk`'s `Inst::Call` intercepts a control signal (it is *not* an error):
  it rewinds `*ip` to re-point at the suspending call — leaving the callee + args on
  the operand stack untouched — and returns `ChunkExit::Suspend { deadline }`.
- `vm_run_bc` turns that into `VmOutcome::Suspended(Suspended { frames, cur,
  entry-marks, deadline })` and returns it **without unwinding**: the operand stack
  and frame slots must survive on the process heap so the resume replays from the
  `%receive` call (a collection while parked relocates them in place; the saved
  `base`/`env_base` indices stay valid). `vm_run_bc` now takes
  `resume: Option<Suspended>` — `Some(s)` restores the registers + frame stack and
  re-enters the loop, on any worker, no coroutine.

**Signatures.** `vm_run_bc` → `Result<VmOutcome, LispError>` (`Done` | `Suspended`,
or a real `Err`). The local `Frame` is promoted to a module `BcFrame` so a
`Suspended` can hold the whole stack. `vm_apply` keeps returning `LispResult`: it
maps `Done`, and on `Suspended` it **re-raises** the control signal (unwinding the
inner roots) — the native-nested `receive` case (§8.1), where the enclosing native
is re-run on resume rather than its inner continuation captured. The three
`vm_apply` callers (`dispatch`, `force`, the IC fast path) are unchanged.

**Why this shape over a coroutine swap.** §7.1: any saved native stack is
thread-affine (cached TLS / return trampolines) — the KI-1b cross-thread-resume
segfault is fundamental to *any* stackful library, not corosensei-specific. Capturing
the continuation as data sidesteps it entirely, and is the same change that buys
live-process migration + fully-precise GC. The survey (§8.1) found the suspending
`receive` is always a clean loop tail across the stdlib, so state-capture covers the
real workloads; the rare native-nested case re-runs.

**Tests.** A capture→resume unit test in `compile.rs` drives `vm_run_bc` with a
suspend-once test native (suspends on call 1, returns 42 on call 2) and asserts:
first run → `Suspended` with the operand stack still rooted; resume → `Done(42)`
with the frame stack torn back down to entry. A `mailbox.rs` test installs a green
ctx (dummy, never-deref'd yielder) + empty mailbox and asserts `receive_match`
returns a `Control::Suspend` — not a real error, and without blocking.

**Green.** Full `make test` passes at the default (the one failure was the
pre-existing reader-depth test `parser_rejects_deeply_nested_input_instead_of_overflowing`,
a native-stack-size flake on this box — passes under `RUST_MIN_STACK=16M`; the
reader is untouched). Differential parity green. §6 plain-release KI-1 bar
re-cleared: `concurrency_race` 10/10 clean + the `BROOD_GC_STRESS` variant, plus
`work_stealing`, all in plain release.

Next (§8.4 step 2): `run_one` dual-mode — store `Run::Suspended(Box<Suspended>)`
and call `vm_run_bc(.., resume)` directly behind the flag, park on the deadline/
mailbox a `Suspended` outcome (the work `wait_for_message` did) — plus the
live-migration regression test (§7.6, resume on a *different* worker).

## 2026-06-08 — corosensei removal §8.4 step 2: dual-mode run_one + live process migration

Wired the step-1 capture machinery into the scheduler and got **live migration** — a
green process resumed mid-computation on a *different* worker than it suspended on,
the thing corosensei's thread-pinned coroutines could never do (KI-1b). Flag-gated
(`BROOD_STATE_CAPTURE`, default **off**), so `main` stays on corosensei.

**Dual-mode `Process`.** `Process.run` is now `Run::{Coro(Coro) | Capture{heap, body,
resume, capture}}`. `spawn` picks the mode: under the flag, a **VM-eligible** 0-arg
body (`process_body_vm_eligible`) runs in capture mode — the worker owns the heap +
body and drives `vm_run_bc` directly, no coroutine; everything else (flag off, or a
body that defers to the tree-walker) keeps a coroutine (§8.1 option a — corosensei
remains only for tree-walked bodies). `run_one` branches: capture mode installs the
`Ctx` for the quantum (no coroutine holds it), drives `run_process_body`, reads the
capture stack back, then dispatches the outcome.

**`vm_run_bc` reifies the scheduler outcomes.** Beyond `Suspended` (step 1) it now
returns, at its loop-top safepoint and only for the top-level body driver
(`top_level && in_capture_run()`), `Preempted(Suspended)` when the reduction budget
hits 0 (`tick_capture`) and `Killed` on a pending hard `:kill` — the state-capture
analogues of the coroutine's `Suspend::{Preempt,Kill}`. A nested `vm_apply` run (a
native callback) passes `top_level=false`, so it never captures these — they're the
body driver's alone.

**`run_one` outcome handling.** `Done` → retire `:normal`; `Err` → retire `[:error …]`
(let-it-crash); `Killed` → retire the mailbox kill reason; `Preempted` → stash the
continuation and re-queue (pinned — locality); `Suspended` → stash + `park_on_receive`
(the same kill-check + raced-message recheck the coroutine path uses). `receive_match`
sets `scanned` and arms the deadline timer before returning the suspend (the park
bookkeeping `wait_for_message` did), and the gate moved from `ctx.yielder.is_some()`
to `in_capture_run()` (capture mode has no yielder).

**Migration falls out, at the right point.** A *woken* capture process (from
`receive`/timer/`exit`, or a message racing its park) is plain `Send` data with no
native stack, so `wake_enqueue` re-routes it to the least-loaded worker
(`migrate_count()` counts the cross-worker moves). A *preempted* process re-enqueues
**pinned** (plain `enqueue`) — migrating a hot, actively-running process every 2000
reductions would only thrash its cache for no benefit; migration is for *idle*
(parked) processes, like BEAM.

**Two correctness fixes found in review.** (1) Capture bodies run on the *worker*
thread stack, not a 16 MiB coroutine stack — so under the flag worker threads are
spawned with a `CORO_STACK_BYTES` stack, else a deep native sub-recursion would
overflow the default ~2 MiB before the `stack_budget` guard (calibrated to the coro
size) trips. (2) A capture-mode `receive` is re-entered from scratch on each wake, so
recomputing `now + ms` reset — and never fired — the `after` timeout; the absolute
deadline is now persisted in the mailbox (`recv_deadline`, cleared on the
match/timeout/error exit).

**Validation.** `tests/live_migration.rs` (the §7.6 acceptance): 200 deep-frame
(~150 non-tail `BcFrame`s) receive/reply processes per burst, suspended at `receive`,
woken, resumed — asserts every result is correct (a mis-restored frame stack would
corrupt it) **and** `migrate_count() > 0` (cross-worker resumes happened). Green under
`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` (heap-verifier walks the live graph each
safepoint — no use-after-GC in the capture/migrate path). §6 plain-release KI-1 bar:
10/10 clean + `BROOD_GC_STRESS`, **flag on and off**. Flag-off in-language suite
1747/1747 standalone (unchanged).

**Step-3 blocker, found and documented (not a step-2 regression — flag is off).** The
flag-on full suite exposes the §8.1 **native-nested-receive footgun**: the re-run model
(a stateful native re-executes its thunk on resume) breaks when the thunk has an
irreversible side effect *before* the receive. Two real shapes: a `:isolated` test that
spawns a worker then receives its reply (`%isolate` would kill the awaited child + spin
its reap-loop → hang; re-raising untouched fixes the hang but the re-run leaks
long-lived children), and a gen-server `call` that mints a fresh `ref` before its
reply-receive (re-run mints a new ref → the prior reply never matches → livelock). The
clean cases all pass. Resolving it — capture the continuation *through* the native
frame so the thunk doesn't re-run, or run a stateful-native-wrapped suspending body on
a coroutine — is the substance of step 3, before the default can flip. Details in
`concurrency-v2.md` §8.1.

## 2026-06-08 — corosensei removal §8.4 steps 3-flip + 4: corosensei is gone

The end of the §8 migration. The default flipped to state capture and **corosensei was
deleted** in the same move — once capture mode is proven correctness-equivalent (the
step-3 entries above), keeping the flag and the coroutine engine around buys nothing.

**What was removed.** The `BROOD_STATE_CAPTURE` flag and `state_capture_enabled()`; the
`Run::{Coro|Capture}` enum on `Process` (now plain fields); the `corosensei` dependency;
the coroutine plumbing — `Suspend`, `Yielder0`/`Coro` types, `build_coro`, `resume_coro`,
`handle_coro_outcome`, the per-suspend `gc_block_save`/`macro_block_save`/`stack_base_save`
dance; and `unsafe impl Send for Process` (the struct is now genuinely `Send`). `run_one`
is capture-only: install ctx → drive `vm_run_bc` → save ctx → finish quantum → handle the
`VmOutcome`. A process body with no compiled 0-arg arm tree-walks **on the worker thread**
and its `receive`s block (the §7.4 dirty carve-out); everything else captures.

**Stealing generalised.** Fresh-only stealing existed solely because corosensei couldn't
resume a saved native stack on another thread (KI-1b). With no native stack, **any** queued
process is migratable, so `try_steal` takes the back-most process (`pop_back`), `STEALABLE`
counts every queued process (incremented in `enqueue`, decremented in `run_one`), and the
`fresh` flag is dropped entirely. `CORO_STACK_BYTES` → `WORKER_STACK_BYTES`.

**Doc/comment sweep.** Every `coroutine`/`corosensei`/`yielder`/`fresh-only` reference in
`scheduler.rs` and the `process.rs` module doc was rewritten to the capture model.

**Regression found and fixed in validation.** `cpu_bound_process_does_not_starve_peers_on_one_worker`
hung: an infinite self-recursive function `(defn hog () (hog))` was never preempted. Root
cause: ADR-069's pass-through optimisation classifies `(defn hog () (hog))` as a thin
wrapper that redirects `hog → hog`, and the redirect loop (in both `compile::dispatch` and
the tree-walker `eval::eval`) relied on `tick()` → `preempt()` to yield. With corosensei
gone, `preempt()` only refreshes the reduction budget — only the VM driver's loop-top
`tick_capture` can actually suspend a process. So the redirect spun forever in a tight Rust
loop *below* any captureable safepoint, monopolising the worker (with one worker, the
responder never ran). Fix: both redirect loops detect a **self-cycle** — a redirect whose
inner head resolves back to the same closure (by closure identity, since a `defn` closure
is anonymous so its global name isn't known at `compute_passthrough` time) — and break,
falling through to the normal call path. The call then runs as a VM `SelfTail`/tail `Call`
whose loop-top reduction check preempts it. The two engines stay in lock-step (the
`differential` corpus test still passes), so the break is mirrored in both.

**Validation.** §6 plain-release KI-1 bar (debug-assertions release): `concurrency_race`
10/10 plain + 5/5 `BROOD_GC_STRESS`, plus `work_stealing`. Via nextest: lib (251),
`differential` (incl. `engines_agree_on_corpus`), `work_stealing`, `live_migration`,
`preemption` — all green. Full `make test`: 553/555, the two failures pre-existing and
environmental — the deep-nest parser test (a stack flake; passes with
`RUST_MIN_STACK=32M`) and the `dist` link-reconnect test (confirmed failing on committed
HEAD too, i.e. with corosensei, so not from this work). Suite runtime is back to ~25 s now
that the coroutine engine and its 16 MiB per-process stacks are gone — the +22% capture
overhead measured under the flag is moot.

**Review pass (same day).** A second read of the diff caught one real bug in the
generalise-stealing change: `drain_worker_queue` (the dirty-block backlog re-route) now
drains the *whole* queue and re-enqueues each process, but each was already counted in
`STEALABLE` at its original `enqueue` — so re-enqueueing double-counted them, inflating
`STEALABLE` by the drained count every dirty block and slowly defeating `try_steal`'s
`== 0` fast-path. Fixed with a `STEALABLE.fetch_sub(stranded.len())` before the
re-enqueue loop (net zero; the count stays equal to the processes actually queued). It's
a hint, not a correctness gate, so the bug only wasted steal scans — but it's now correct.
The rest of the pass was comment hygiene: every lingering `coroutine`/`corosensei`/
`yielder`/`fresh-only`/`BROOD_STATE_CAPTURE` reference across `scheduler.rs`, `mailbox.rs`,
`compile.rs`, `process.rs`, `cli_support.rs`, `Cargo.toml`, and `live_migration.rs`
rewritten to the (flagless, coroutine-free) state-capture model. Re-validated: §6 bar
10/10 + 5/5 GC-stress; the affected nextest suites green; the in-language suite passes in
~43 s **with commit signing disabled** — the apparent "regression" during review was the
package-manager test's `git commit` blocking on SSH-signing through a locked 1Password
agent (a known environmental hang, KI in `next-up-schedulers` memory), not a code fault.
Benchmarks archived (`docs/benchmarks/2026-06-08T15-28-17Z.md`): VM/tree-walker ratios
unchanged and strong (fib25 7.7×, sum_tail-100k 9.7×, reduce_range-1M 3.8×); scheduler
fan-out healthy (`spawn_fanout` 1000 = 4.6 ms).

## 2026-06-08  stdlib expansion — path, system, crypto, agent, enum extras

**Five new opt-in modules added to `std/`.**

`std/path.blsp` (`require 'path`) — pure path-string manipulation: `join`, `split`,
`basename`, `dirname`, `extension`, `stem`, `normalize` (resolves `.`/`..`), `relative-to`,
`absolute?`, `with-extension`. No filesystem calls except `exists?`/`is-file?`/`is-dir?`.

`std/system.blsp` (`require 'system`) — OS interaction over new Rust primitives: `env`
(single var or nil), `env-all` (all env vars as a map), `argv` (vector of arg strings),
`os-type` (`:linux`/`:macos`/`:windows`/`:unknown`), `cmd`/`cmd-ok?`/`cmd-out` (run a
subprocess, capture stdout/stderr/exit), `working-dir`/`host` (aliases for builtins to
avoid name collision), `halt`.

`std/crypto.blsp` (`require 'crypto`) — ChaCha20-Poly1305 AEAD (`encrypt`/`decrypt` on
byte vectors, `encrypt-str`/`decrypt-str` for string convenience; wrong key/nonce returns
`:error`), `random-bytes`/`random-key`/`random-nonce`, PBKDF2-HMAC-SHA256 (`pbkdf2`),
`secure=?` (constant-time byte-vector equality). PBKDF2 implemented manually over
`hmac 0.13` (the `pbkdf2 0.12` crate uses `digest 0.10` which conflicts with our
`sha2 0.11`/`digest 0.11`).

`std/agent.blsp` (`require 'agent`) — Erlang/Elixir-style process-backed state cell:
`start`/`start-link` (spawn loop holding state), `get`/`update`/`get-and-update`/`cast`/
`stop`. Uses `spawn`+`send`+`receive`+`ref` — all Brood, no new primitives. Bug fixed
during testing: `(spawn (fn () body))` double-wraps the body (the `spawn` macro already
adds `(fn () …)`); the correct form is `(spawn body)`.

**Enum extras in `std/prelude.blsp`:** `chunk-every` (partition into fixed-size chunks,
keeping remainder), `chunk-by` (partition by consecutive key), `scan`/`reductions`
(running fold returning all intermediate values), `flat-map` (alias for `mapcat`),
`zip-with` (combine two sequences element-wise), `intersperse` (alias for `interpose`),
`min-by`/`max-by` (extremum by key function).

**Tests:** `tests/path_test.blsp` (46), `tests/system_test.blsp` (19),
`tests/crypto_test.blsp` (22), `tests/agent_test.blsp` (10),
`tests/prelude_enum_test.blsp` (30). Two bugs found and fixed during test runs:
`path/basename "/"` returned `""` instead of `"/"` (edge case in last-sep scan);
`path/with-extension` on a bare filename prepended `"./foo.md"` instead of `"foo.md"`
(dirname returns `"."` for bare names — fixed by skipping the prefix when dirname is `"."`).
Also corrected a PBKDF2 test vector (comment said `120fb6cffccd…` but the correct SHA256
output for password/salt/1iter is `120fb6cffcf8b32c…`).

**Suite result:** 553/555, same pre-existing failures as before (parser deep-nest SIGABRT,
dist link-reconnect flake).

## 2026-06-08 — HMAC primitives: ~200x speedup for hmac-sha256/sha1/sha512

**Motivation:** the stdlib benchmark (`docs/benchmarks/2026-06-08T17-36-20Z.md`) showed
`hash/hmac-sha256` at ~1.94ms/call vs `hash/sha256` at ~9µs/call — a ~200x gap. Root
cause: the Brood RFC 2104 construction in `std/hash.blsp` round-tripped through hex
(`%sha256-bytes` returns a hex string, so the inner-hash bytes had to be decoded back via
`hash--hex->bytes-loop` before the outer hash). That loop + the 64-element `map` XOR key
pads + multiple `append`/`into []` conversions added up.

**Fix:** added `%hmac-sha256`, `%hmac-sha1`, `%hmac-sha512` as Rust builtins in
`crates/lisp/src/builtins.rs` over the `hmac 0.13` crate already present (used by the
node-link handshake and PBKDF2). These are justified as crypto primitives — the `hmac` crate
is already a hard dependency, and the performance gap was caused by an API mismatch (hash
returning hex, not raw bytes) rather than intrinsic Brood list-processing overhead.

`std/hash.blsp` now delegates `hmac-sha256`/`hmac-sha1`/`hmac-sha512` to the primitives,
removing `hash--hmac`, `hash--normalize-key`, `hash--xor-pad`, `hash--hex-nibble`,
`hash--hex->bytes`, `hash--hex->bytes-loop` from the module.

**Benchmark result:** `hmac_sha256 × 50` → median ~521µs (10.4µs/call), vs 1940µs before
— ~190x faster, now comparable to `sha256` (~9µs/call). All 43 hash tests pass.

## 2026-06-08 — JIT Stage 1 landed (tier-1 template JIT via Cranelift, ADR-101)

Implemented Stage 1 of the JIT (`docs/jit-stage1.md` §7, roadmap) behind `--features jit`,
off by default (zero cranelift linked, zero cost when absent). The first real codegen: a
hot RUNTIME-region arm's **dispatch-bound int subset** — `Const(Int)`, `Local`,
`Prim2{Add,Sub,Mul,Lt,Le,Eq}`, `JumpIfFalse`/`Jump`, `SelfCall` — lowers from its bytecode
`Chunk` to Cranelift IR (`jit_lower_arm` in `eval/compile.rs`): block leaders + a depth
worklist with block-param merges, the operand stack virtualised at compile time (so `roots`
never grows), the self-loop as a back-edge calling `brood_rt_tick` for preemption. `brif`
fires only on the `I8` result of a comparison prim (Brood Int `0` is truthy, so a raw
payload can't drive a branch). Anything outside the subset bails the whole compile — the
arm stays on the VM.

**Tiering:** each `CompiledArm` carries `jit_calls: AtomicU32` + `jit_code: AtomicPtr<u8>`;
the 8th call compiles under the process-wide `GLOBAL_JIT` mutex and installs the finalized
pointer atomically (late-binding-safe: a `def` epoch bump invalidates as it does the VM IC).
`BAILED` (≠ null, ≠ a real pointer) marks an out-of-subset arm so it's tried once.
**Two VM hooks run it:** `vm_run_bc`'s fresh-start path and the `ChunkExit::Call` site (so a
hot Brood→Brood callee runs native too), each deopting to the VM with the frame stack intact
(codes 0=Done / 1=non-int / 2=preempt). The pinned-register trampoline (ADR-101 §6.2) was
**not needed** — callbacks take `heap` as a normal arg, so no hand-written asm.

**Repr decision held** (`value-repr.md`): `Value` stays the 16-byte `#[repr(C, u8)]` enum;
GC-visible values live in `Heap::roots` between callbacks, only unboxed `i64` rides in
registers (no stack maps — the moving collector can't see into a JIT segment, so it mustn't
need to). `value_layout_is_stable_for_the_jit` pins the offsets as a compile-time ABI guard.

**Verified (all `--features jit`):** differential JIT≡VM 2/2; lib 258/258 (+6 JIT tests);
in-language suite 2039/2039; §6 KI-1 bar — `concurrency_race` 10/10 under
`BROOD_GC_STRESS=1`, built `RUSTFLAGS="-C debug-assertions=on" --release`. **~27× speedup**
on a `sumto` int loop (`jit_speedup_vs_vm`, `#[ignore]` bench). Also forwarded the `jit`
feature through the `cli` crate (`cargo run -p cli --features jit`) for single-file
iteration.

**Speedup:** `jit_speedup_vs_vm` measures **~65×** on `sumto(100000,0)` (VM ~18s vs JIT
~0.28s over 300 reps).

## 2026-06-08 — JIT: compile on a background thread (scheduler-starvation fix)

The first cut of Stage 1 compiled arms **synchronously on the worker thread**, holding
`GLOBAL_JIT`. That surfaced as a flaky in-language-suite miss: a test would occasionally
fail on a tight `(after ms …)` / monitor `:down` wait. "No stone unturned" — so I
reproduced the root cause deterministically rather than bumping the timeout.

**Repro:** an env-gated `BROOD_JIT_COMPILE_DELAY_MS` sleep inside the synchronous compile
path. With 50ms, the full suite failed reliably — and *not always on the same test*
(`dynamic_test` one run, `json across processes` the next). That's the tell: the bug is
**general scheduler starvation**, not a JIT logic bug. Cranelift codegen is CPU-bound work
of non-trivial duration; doing it inline under the lock means that during a compile burst
the whole worker pool serializes on `GLOBAL_JIT`, and any process blocked on a tight timer
misses its deadline. (The amplified run took 326s.)

**Fix:** a single dedicated `brood-jit` background thread is now the *only* place arms are
lowered — and the only holder of `GLOBAL_JIT`, so that lock is otherwise uncontended.
Worker threads never compile: `jit_tier` counts calls, and on crossing the threshold a
`null → QUEUED` CAS elects one thread to hand the `Arc<CompiledArm>` to a bounded channel
(`sync_channel(256)`); everyone runs the VM until the background thread installs the native
pointer (or `BAILED`). A full queue resets the arm to untried so it re-tiers later. New
`QUEUED` (2) sentinel alongside `BAILED` (1).

**Proof the fix addresses the mechanism (not luck):** move the same `BROOD_JIT_COMPILE_DELAY_MS=50`
sleep onto the background thread and re-run — the suite passes 2/2 and finishes in ~55s
(vs 326s + failures synchronously), because the delay no longer touches the workers. Then
removed the knob.

`jit_tier` now takes `&Arc<CompiledArm>` (both VM hooks already pass an `Arc`); the tiering
unit tests poll for the now-async compile, and the speedup bench warms by polling — which
also made it reliably catch the native path, so the measured speedup rose from ~27× to
**~65×**. Verified post-fix (and after merging `main`, which brought the "emit `SelfCall`
for `defn` tail self-calls" change the JIT lowers): differential 2/2, lib 259/259,
in-language suite green across repeated runs, §6 KI-1 bar 10/10 + `GC_STRESS`.

## 2026-06-09 — JIT Stage 1.5/2: fire on real fused code, + 4 correctness fixes

`jit_lower_arm` only handled the *unfused* `Inst::Prim2`, but `emit_node` fuses the
common loop-body shapes — `(- i 1)` → `Prim2SlotInt`, `(+ acc i)` → `Prim2SlotSlot` — so
the JIT **never fired on a real compiled int loop** (every realistic arm bailed). Lowered
both fused variants (read the operands from frame slots / a literal instead of the operand
stack; net +1 stack depth) so the JIT now tiers `sumto`/`down`/collatz/fib-accumulator
bodies. Confirmed end-to-end with a compile trace: real `prod`/`classify` arms tier and run
native.

Four correctness fixes came with the wider coverage (the JIT now fires far more, so latent
edges became reachable):

1. **`map` was ignored.** The old lowering did `iadd(aa, bb)` without applying the
   passthrough arg-map, so `(> a b)` (`%lt`, `map = [1,0]`) computed `a < b`. Now `map` is
   applied for both fused and unfused prims (the `pick` helper).
2. **Arithmetic wrapped instead of promoting.** `iadd`/`isub`/`imul` silently wrapped on
   i64 overflow, where the VM defers to the native and promotes to a BigInt. Now uses
   `sadd_overflow`/`ssub_overflow`/`smul_overflow` and **deopts** to the VM on overflow, so
   an accumulating product yields the same BigInt (`(prod 30 1)` → `30!`).
3. **Operator redefinition after tiering diverged.** A tiered arm inlines `+`/`<`/… as raw
   machine ops; a `(def + …)` afterwards left it computing the old op (JIT gave 6, VM 5).
   Added the epoch-guard: `CompiledArm` carries the `global_epoch` it was compiled at; a
   JIT'd arm evaluates no Brood so no `def` can land mid-run, so `jit_tier` compares the
   live epoch once per *activation* and, on a mismatch, invalidates + re-tiers
   (re-validating operators via `chunk_ops_all_native` — recompiles if an unrelated global
   moved, bails permanently if the operator itself was redefined). Self-heals after an
   unrelated `def`; honors a real redefinition.
4. **Pre-existing VM bug, surfaced while testing.** The fused `Prim2SlotInt` `(op Const
   Local)` case stores the operands swapped (local as `slot_a`, const as `int_b`, inverted
   `map`); the *inline* path uses `map` correctly, but the slow-path **dispatch fallback**
   pushed `[sa, sb]` raw, so `(/ 24 x)` (x=5, inexact → dispatch) silently ran as `(/ 5 24)`
   = `5/24` on the VM (the tree-walker gave the right `24/5`). Added a `swapped` flag (free —
   `Inst` stays 56 bytes) and the fallback un-swaps to the original call order. This was a
   VM≠tree-walker divergence the differential corpus missed; added regression entries.

Also extended the int subset with the **integer division family** (`rem`/`quot`/`%div`):
Cranelift `sdiv`/`srem` *trap* on a zero divisor and on `i64::MIN / -1`, so both are guarded
→ deopt before the op (matching the VM's defer-to-native), and `%div` deopts on a non-exact
quotient (the VM returns a Float there). Enables native collatz/`mod` loops.

**Tests:** +1 unit test (fused/map/overflow lowering), the two tier tests + the speedup
bench reworked to use a prelude-loaded heap (so the new operator-validation resolves
`+`/`-`/`<`), a new `tests/jit.rs` (13 end-to-end cases: fused loops, overflow→BigInt,
every comparison + map, negatives, non-int deopt, redefinition, self-heal, nested ifs,
division family, div-by-zero/MIN-overflow deopt), and 5 differential-corpus regression
entries for the `Prim2SlotInt` order bug. Full suite green: 411/411 with `--features jit`
(incl. the in-language suite, GC-stress, concurrency_race), 391/391 default;
`tests/jit.rs` green under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`. `make install` already
defaults the JIT on (`WITH_JIT ?= 1`, `./configure --without-jit` to opt out).

## 2026-06-09 — JIT tier-2 foundation: hybrid operand model (handles in roots)

Toward making the JIT fire on *real* code (which is full of function calls and list
ops, not just self-contained arithmetic loops). The keystone is letting an operand-stack
slot hold a **heap handle** in `roots` (GC-visible), not just an unboxed `i64` in a
register. Landed and committed (each green + `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`):

- **let-bindings** (`SetLocal`/`Pop`): `(let …)`/`(do …)` bodies JIT (lazy slot store +
  a copy/tag-check on read; deopt-re-run-from-ip-0 stays correct because let-slots are
  scratch, recomputed before use, distinct from the loop-carried param slots).
- **return-via-roots**: the `Done` block returns through `roots[base]` (each exit stores
  its result there) instead of an `i64` block param — so a returned value can be a handle.
- **hybrid operand model**: the operand stack is `Vec<Op>` with `Op = Int(reg) | Slot(k)`.
  `Local` pushes a lazy `Slot`; arithmetic/branches tag-check it to an int, while a binder
  / self-call arg / return copies the whole `Value` verbatim, so a handle round-trips
  untouched and stays in `roots` where the moving GC sees it. SelfCall reads all args into
  registers before storing (so `(f b a)` doesn't alias). Handle-carrying / -returning /
  -`let` arms now JIT (verified: a loop carrying a list through and returning it runs
  native and matches the VM).
- **24-byte-`Value` bugfix**: `Value` is **24 bytes**, not 16 — `Pid { node, id }` needs
  two payload words (`id` at offset 16). The first cut of the handle copy moved only
  tag+payload (offsets 0/8), so it would corrupt a `Pid` moved between slots or returned
  (the Pair-based tests passed because a Pair only uses offset 8). Now copies every word;
  layout test pins `size == 24` and `Value::Pair`'s discriminant `== 9` (`TAG_PAIR`; note
  it's 9, not `Tag::Pair`'s 7 — `Value` has an extra `BigInt` after `Int` and a `Rope`
  before `Pair`).

GC discipline that makes this sound: a handle lives in `roots` (a frame slot) across the
only safepoint (the loop back-edge), and `collect` relocates roots **in place** (never
reallocates the Vec), so the JIT's cached `roots_base` stays valid.

**Next (cons/car/cdr, then calls) — the worked-out plan.** These produce/consume *new*
handles, so they need an `Op::Handle(w0,w1,w2)` (a `Value` as three registers). Because a
`Value` is 24 bytes it can't be a register-pair return, so the runtime callbacks
(`brood_rt_cons`/`car`/`cdr`, and later `call_slow`) use an **out-pointer ABI**: the JIT
passes a stack-slot address, the callback writes the result `Value` there, the JIT reads
the 3 words back. `car`/`cdr` tag-check `Pair` first (deopt on non-pair, incl. nil).
cons-allocating loops emit a back-edge `brood_rt_gc_safepoint` to bound the nursery (safe:
the operand stack is empty there, so no handle is in a register across the collection).
A fresh handle never crosses a block boundary in the target patterns (it's consumed
in-block by SelfCall/Done), so bail if one would. `alloc_pair` only grows the nursery
(never collects), so reconstructed operand `Value`s can't go stale mid-`cons`.

## 2026-06-09 — JIT: cons / car / cdr land (the JIT fires on list code)

Built on the hybrid-operand-model foundation: `cons`/`first`/`rest` now compile, so the
JIT accelerates real list code, not just arithmetic. These produce/consume heap handles,
which can't ride in a register across a safepoint, so they use runtime callbacks with a
by-value **out-pointer ABI** (a `Value` is 24 bytes → can't be a C register-pair return):
the JIT passes a scratch stack-slot address, the callback writes the result `Value` there,
and the JIT reads the three words back into a new `Op::Handle(w0,w1,w2)`.

- `cons` (generic `Prim2{Cons}` + `(Local,Local)`-fused `Prim2SlotSlot{Cons}`): car =
  source 0, cdr = source 1. It allocates, so cons arms emit a back-edge
  `brood_rt_gc_safepoint` to bound the nursery — safe there because the operand stack is
  empty (no handle in a register across the collection) and `collect` relocates the frame
  slots in place, so `roots_base` stays valid. `(_,Const)`-fused `Prim2SlotInt{Cons}` bails.
- `first`/`rest` (`Prim1`): tag-check the operand is a `Pair` (`TAG_PAIR` = 9; deopt to the
  VM on a non-pair — nil, type error), then read car/cdr via the callback.
- `Op::Handle` is transient: produced and consumed within a block (stored whole — 3 words —
  into a slot by a self-call arg / binder / return, or tag-checked back to an int when used
  as a number), never crossing the loop back-edge live, so the GC never sees a handle in a
  register. `alloc_pair` only grows the nursery (never collects), so reconstructed operand
  `Value`s can't go stale mid-`cons`.

Verified: list build, computed-car cons, `nth` via first/rest, `(+ (first xs) …)`,
walk-to-nil, build-then-traverse — all match the VM and pass under `BROOD_GC_STRESS=1
BROOD_GC_VERIFY=1` (the cons loops allocate a pair per iteration, so the verifier walks the
live graph at every collection — the real proof the handles-in-roots / handles-only-
transiently-in-registers discipline holds). Commits `08844a4`/`b6ba590`/`3564a39`/`19c4333`.

**Remaining JIT payoff: Brood→Brood calls** (`Inst::Call` via `brood_rt_call_slow`) — so a
body that calls a *helper* JITs (the common real-code shape). Reuses `Op::Handle` for the
result + the out-pointer ABI pattern; needs deopt/preempt/error handling across the call.

## 2026-06-10 — Kernel review: two bugs fixed (timer wakeup, prim2 de-opt) + cleanup

Reviewed the VM, GC, and scheduler for bugs / cleanup / speedups. Found two genuine bugs,
both independent of any in-flight JIT work, plus hardening. Full suite stays green.

**Bug 1 — lost timer wakeup in the `receive` suspend→park window** (`process/`). A
capture-mode `receive` *arms* its `(after ms …)` timer (`mailbox.rs`) before the process is
actually parked (`scheduler.rs::park_on_receive`), which only happens after the suspend
signal travels back up through `vm_run_bc`/`run_one`. If the deadline fired inside that
window, `wake_for_timeout` found no `waiter`, returned without re-queuing, and *consumed*
its (current-gen) timer entry — then `park_on_receive` parked the process forever (it only
re-checked `kill_pending` + a raced message, never the deadline). Invisible at the suite's
multi-second timeouts; a real lost-wakeup for sub-ms deadlines under load. Fix:
`park_on_receive` re-checks `st.recv_deadline` under the `mailbox.state` lock it already
holds and re-queues on a passed deadline, so the timer-fire and this check serialise — one
of them always wakes the process.

**Bug 2 — swapped `Prim2SlotInt` permanently de-opted after any `def`** (`eval/compile.rs`).
A `(op Const Local)` fusion (`(- 24 x)`, `(/ 100 x)`, `(< 5 x)`) stores an *inverted*
arg-map so the inline operand pick stays correct. But `prim2_inline_exec`'s guard
revalidation compared that inverted map against `resolve_prim`'s *natural* map, which never
matches — so after the first epoch bump (any `def`) the arm fell to the slow dispatch path
*and* re-ran `resolve_prim` every call, forever. Results were always correct (the slow path
un-swaps); only the inline fast path was silently lost — exactly the de-opt the fusion
exists to prevent, biting the REPL/hot-reload workflow. Fix: pass `swapped` so revalidation
un-inverts the map first. Regression test `swapped_prim2slotint_reinlines_after_epoch_bump`.

**Cleanup / hardening.** Deleted dead `Heap::set_root` (duplicated `set_root_at`, no
callers) and the dead `EXIT_REASON` thread-local (never read). Relaxed the per-quantum
`RUNNING`/`PEAK_RUNNING` diagnostic atomics from `SeqCst` to `Relaxed`. Made the
tree-walker-defer `capture_top_level` save/restore panic-safe via an RAII `CaptureTopGuard`.
Extended `value_is_immovable` to check `Range`/`Transient` (tripwire gap) and added a
`transients.is_empty()` debug-assert to `freeze_as_shared_code` (matching the rope one).

## 2026-06-10 — JIT tier-2: Brood→Brood calls (non-tail + tail-call TCO)

The JIT now fires on bodies that call other functions — the common real-code shape —
both non-tail (a helper call whose result feeds the body) and tail (mutual recursion /
a body ending in a call to a *different* global). This was uncommitted working-tree work
that an external `git reset` wiped mid-session; reconstructed from the captured diff and
re-validated.

**Mechanism.** A `Value` never rides in a register (24-byte enum), so a call stages its
callee + args onto the operand stack (`roots`) via `brood_rt_push` in the VM's `Inst::Call`
layout, then:
- **Non-tail** → `brood_rt_call_slow` → `jit_dispatch_call`: runs the callee inline (a
  nested, non-top-level VM apply, so it can't preempt/suspend across the native boundary —
  the §7.4 dirty carve-out), result read back as an `Op::Handle` via the out-pointer ABI.
- **Tail** → outcome 4 → `jit_dispatch_tail`: hands `vm_run_bc` a `ChunkExit::Tail`/`Done`
  so the driver reuses the frame (TCO). The native stack never grows — the driver loop is
  the trampoline — so 2M-deep mutual recursion stays O(1) stack.
- A free-global callee resolves live via `brood_rt_global` (`jit_resolve_global`), reading
  the current env so a `def` rebind is seen immediately (late binding). Unbound → an error
  parked in `JIT_PENDING_ERROR`, propagated as outcome 3 (no VM re-run).

**GC discipline.** Staged operands are real `roots` (GC-visible). A live `Handle` left
*below* a call's operands would be a bare heap pointer in a register across the callee's
collection, so the lowering **bails** if any deeper operand is a `Handle`; `roots_base` is
re-fetched after every call (a `push`/callee frame may reallocate it, so it's an SSA
`Variable`, not a fixed value). Comparison results box as `Value::Bool` (`TAG_BOOL`), not
`Int 1` (`box_scalar`).

**Body-weight gate.** A tail call round-trips native↔driver per hop; benchmarking puts the
crossover at ~4 work ops, so an arm *ending* in a tail call lowers only with ≥4 work
instructions — a thinner ping/pong stays on the VM (same speed, no regression). Plain
`SelfCall` int loops (no round-trip) still tier (~27×). `opt_level=speed` on for GVN +
redundant-load elimination across the re-read frame slots.

**Validated.** `breakage/jit_breakage_test.blsp` — 37 warmed JIT≡VM cases (non-tail/tail
calls, mutual recursion, 2M-deep O(1) stack, handle-valued tail args via `cons`,
cross-process shared native code, comparison bools, deopt boundaries) — green plain and
under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`, plus the lowering unit test and the full
workspace suite.

## 2026-06-13 — Persistent child processes: `proc-spawn`/`proc-send`/`proc-close` (ADR-104)

The language gap behind myedit's multi-source completion: an LSP source needs a
*long-lived* child spoken to in framed JSON-RPC over stdio, and `%os-cmd` only runs
a child to completion. Added a persistent-child primitive — and, true to the
mechanism already in the tree, it's the socket model (ADR-062) wearing a different
hat, not a fatter `%os-cmd`.

A child is the same shape as a TCP stream — a bidirectional byte channel whose reads
mustn't pin a scheduler worker — so it reuses the blocking-IO → mailbox seam
(ADR-059, `spawn_io_source`). New `crate::proc` mirrors `crate::net`: stdout and
stderr each read on a non-worker thread that delivers to the owning process's
mailbox; Brood just `receive`s.

- `(proc-spawn prog args)` → a `Value::Subprocess` handle; throws if the program
  can't start.
- stdout → `[:proc handle data]`; stderr → `[:proc-err handle data]` (**separate** —
  merging would corrupt JSON-RPC framing); exit → `[:proc-closed handle code]`
  (`code` is the exit status, or `nil` if signalled).
- `(proc-send p s)` writes `s` to stdin + flushes; `(proc-close p)` kills + drops
  stdin (idempotent; the final `[:proc-closed …]` still reaches the owner).

A new 19th `Tag` (`subprocess`) threaded through `value.rs` / `types.rs` /
`message.rs` (+ dist-wire rejection) / printer / heap hash+equality+ordering / the
MCP JSON bridge — the standard cost of a scalar handle, paid once and consistent
with `Socket`. Chose the dedicated handle over a reused int/ref: it's type-safe
(`expect_subprocess`) and must round-trip through messages anyway (the reader
threads tag their `[:proc …]` with it, and it may cross a `send`/`spawn` since the
registry is runtime-global — `proc-send` works from any process, output still lands
in the owner's mailbox).

Two implementation knots worth noting: (1) stdin lives behind its *own*
`Arc<Mutex<…>>`, not under the registry lock — a blocking write to a child that
never drains its stdin must not stall every other `proc-*` op (a `ChildStdin` can't
be `try_clone`d like a `TcpStream`). (2) Exactly one waiter reaps the child: the
stdout reader, after EOF, polls `try_wait` with a brief lock + short nap, so it
never holds the lock while blocked and a concurrent `proc-close` `kill` can always
take it. Text-only like sockets (`from_utf8_lossy`) — fine for JSON-RPC.

`tests/proc_test.blsp`: 5 isolated cases — handle/`type-of`, `cat` stdin echo +
signal-death `nil` code, `sh -c` stdout + exit code 3, stderr as a separate channel,
and the handle crossing into a worker process. Green; full workspace suite + 257
lib tests still green. Editor-side wiring (an LSP completion source) is the next
step, in myedit — no further kernel work needed.

**Follow-up (consumer shipped).** myedit's `src/lsp.blsp` — an LSP client + multi-source
completion source — now runs on `proc-*` end to end: `M-x lsp-connect` spawns
rust-analyzer and Tab returns its 63 completions for `String::ne…`, merged with the
mode/buffer-word sources. Building it surfaced one real limitation of the text delivery:
the reader's per-chunk `from_utf8_lossy` (proc.rs / net.rs) mangles a multi-byte char
split across a read boundary, and since that changes the byte count it can desync a
byte-`Content-Length`-framed protocol for responses larger than one 64 KiB read. The
client frames byte-accurately (`string->utf8-bytes`) so small/medium responses are exact;
a *fully* faithful client wants `proc` to deliver **raw bytes** — reinforcing the
bytes-value-kind roadmap item already flagged here and in net.rs. Not needed for the
common case; noted as the next proc increment when a byte-framed protocol needs it.

## 2026-06-14 — JIT: two small codegen wins from a cross-language benchmark audit

Profiling the [`brood-benchmarks`](../../brood-benchmarks) suite (isolated `taskset`
re-run on `whklat`) pinned two benchmarks to the **bytecode VM** that should tier,
both one-line-ish JIT-subset gaps — the machinery already existed.

- **Bool consts in the lowering subset** (`613c484`). `chunk_in_jit_subset` admitted
  `Int`/`Nil`/`Float` constants but not `Bool`, so a self-tail loop whose exit arm
  returns a `true`/`false` literal (e.g. `primes`' `divides-none?` via `cond`) bailed
  to the VM and never tiered. The codegen already had `Op::Bool` + `bool_param`
  block-arg tracking + Bool handling in `read_words`/`as_int`/`JumpIfFalse` (from the
  float-JIT work); only the subset gate and the `Const(Bool)` lowering case were
  missing. Result: **primes 351 → 43 ms (~8×, last → 3rd of six)**; also tiers
  `nqueens`' bool `safe?` arms (**933 → 512 ms**). A 30M-iter bool-returning loop:
  ~1.4 s → 0.28 s, identical to the int form.

- **N-ary `+`/`*` left-fold** (`dcb4232`). `resolve_prim` inlines a *2-arg* `+`/`*` to
  a native `Prim2`, but a 3+-arg call (`bintree`'s `(+ 1 (check …) (check …))`) fell
  through to the variadic prelude `fold`, keeping the arm off the native path. Now an
  n-ary call whose head is a free reference to the prelude `+`/`*` (associative, map
  `[0,1]`) left-folds into nested `Prim2(Add/Mul)` — each step deopting on i64 overflow
  exactly as `%add`/`%mul` promote to BigInt, so results are bit-identical. Result:
  **bintree 1123 → 452 ms (~2.4×)**. Comparisons (`<`/`=` chain pairwise) and swapped
  wrappers (`>`) are excluded.

Verified: per-benchmark JIT == tree-walker parity across the suite, BigInt/float/10-arg
n-ary cases match, `jit_cons_test` green. **Not fixed:** `matmul` (~100×) — its inner
`nth` loop under-tiers in a *data-dependent* way (a deopt, not a missing codegen path);
under investigation with a new `jit_native`/`jit_deopt`/`jit_preempt` perf counter.

Aside (caught during this work): the full test suite isn't run with `--features jit`
by default (`make test` builds default features), so the JIT path is uncovered in
normal CI — and under `--features jit` the in-language suite is flaky run-to-run
(different unrelated tests fail each run; two `jit_*` unit tests are stale-red on
clean `main`). Worth a dedicated CI lane + a flake/staleness sweep.

## 2026-06-14 — JIT: top-level-lambda promotion (pipeline ~4.1×, matmul ~2.2×)

Continuing the benchmark audit, two more rows were pinned to the **tree-walker** —
`pipeline` (552 ms, last by a wide margin) and `matmul`'s matrix construction. The
cause was the same: a top-level inline `(fn …)` literal. `pipeline`'s `filter`/`map`
stages (`(->> (range n) (filter (fn …)) (map (fn …)) (reduce + 0))`) and `matmul`'s
`(into [] (map (fn (i) … (map (fn (j) …))) …))` are `(fn …)` literals written directly
in a *top-level* form. Their body (`fn_rest`) sits on the movable **LOCAL** data heap,
so `compile_make_closure`'s `fn_rest_is_stable` guard bailed (a movable handle can't be
baked into a `MakeClosure` node), and the **whole enclosing form** deferred to the
tree-walker (perf-stats confirmed: `pipeline` ran with `jit_native=0`, `tw_defer=1`).

- **Promote a top-level lambda's `fn_rest` into RUNTIME** (`dfa4f67`). When `fn_rest` is
  LOCAL, `heap.promote()` it into the immovable RUNTIME code region (exactly what
  `const_node` does for a literal), so the form is VM-compilable and tiers. Gated on
  `heap.gc_enabled()` — **runtime heap only**. During the prelude *build* (gc disabled)
  a macro/`defn` closure's `fn_rest` is *also* LOCAL here but is promoted by its own
  `def`; promoting it now corrupts it mid-construction (an earlier ungated attempt
  crashed universally — `defn`'s `& body` rest param went unbound), so the build path
  defers exactly as before. The baked RUNTIME handle is rewritten in place under a
  compaction, like every other `MakeClosure`. Result: **pipeline 552 → 134 ms (~4.1×)**,
  **matmul 542 → 243 ms (~2.2×)** — one fix, two rows. (`matmul`'s inner `nth` multiply
  loop still under-tiers data-dependently; that's the residual gap.)

Verified: prelude + macros intact (`defn`/`& body` work), per-benchmark JIT ==
tree-walker parity (matching checksums across the suite), `make test` green, the
`format` tiering regression test green, full `--features jit` suite green.

Also closed the stale-test aside from the entry above:

- **Two stale `jit_*` unit tests fixed** (`6a66673`). Both reflect *deliberate*
  committed behaviour, not regressions (real `fib`/`primes` still tier — `jit_native`
  > 0, `jit_deopt = 0`), and went red unseen because `make test` runs without
  `--features jit`. (1) The bail example was a `Const(Nil)` arm, but the bool/nil/float
  subset admission (`9dfc00f`) made scalar `Const`s in-subset — switched it to a
  `MakeMap(0)` arm (map-build has no lowering path). (2) A test asserted a *free-global*
  tail call lowers, but `jit_lower_arm` now bails those by design (`jit_dispatch_tail`
  reads a staged callee an elided head never leaves — the common mutual-recursion shape
  stays on the correct VM path); rewrote it to assert the *computed*-callee tail call
  lowers and pinned the free-global bail as an explicit counter-case.
- **`set!` foreign-construct hint test → the write-time checker.** In `(set! x 1)` both
  the head (`set!`) and the arg (`x`) are unbound; the bytecode VM's call-head elision
  resolves the head *after* the args, so the runtime error reports `x`, not `set!`
  (the tree-walker reports the head). Rather than reorder the hot call path, the test
  now asserts `(check '(set! x 1))` — the robust, engine-independent guidance surface;
  `(loop 1)` (literal arg) stays as the runtime example.

## 2026-06-14 — `proc-spawn` options map: `:cwd` + `:env` (ADR-104 update)

Surfaced by myedit's project shell (`C-x p e`): commands like `nest format` must run
*in the project root*, but `proc-spawn` took only `prog` + `args` and a child always
inherited the editor's working directory. The prime-directive fix is in the language,
not a `sh -c "cd <root> && …"` wrapper in the editor — so `proc-spawn` grew an optional
third argument, an options map `{:cwd "dir" :env {"K" "V" …}}`. `:cwd` →
`Command::current_dir`; `:env` adds variables on top of the inherited environment.
Arity is now `range(2, 3)`; an absent map (or `nil` `:cwd`) keeps the old behaviour, so
every existing caller is unchanged. `crate::proc::spawn` took `cwd: Option<&str>` +
`env: &[(String, String)]` params; the builtin parses the map with `heap.map_get` /
`heap.map_entries`. Tests in `tests/proc_test.blsp`: `pwd` under `:cwd "/"` reports `/`,
and a var set via `:env` is visible to the child. Both general knobs LSP servers and the
web mirror will want too.

## 2026-06-14 — LSP: hover + goto on `defmodule` `:use`/`:alias`/`:implements` clauses

Closed a gap in the editor surface: the module/behaviour *names in a `defmodule`
header* had no hover and no goto target. They bind nothing (a module isn't a
value), so scope analysis resolves them `Free` and the generic hover/goto paths
rendered nothing. Added `crates/lsp/src/module_ref.rs` — `clause_ref_at` recognizes
them structurally from the CST (the form right after a `:use`/`:alias`/
`:implements` keyword), shared by both `hover.rs` and `definition.rs`:
- `(:use foo)` / `(:alias foo)` → **goto** jumps to `foo.blsp` via
  `introspect::module_file` (same `require--find` lookup `require` uses); **hover**
  shows `(module foo)` + the docstring its `defmodule` declared, via a new
  `introspect::module_doc` (reads `*module-docs*`).
- `(:implements Bar)` → **goto** scans the project's files
  (`introspect::project_files`) for `(defbehaviour Bar …)`/`(defprotocol Bar …)`
  and lands on its name — the interface registry (`*protocols*`) records ops but no
  def-site, so there's nothing to ask `source-location`. **Hover** shows
  `(behaviour Bar)` + its ops/arities via `introspect::protocol_ops`. A behaviour
  living only in an external package (e.g. hatch's `protocol.blsp`) has no goto
  target; hover still lists its ops when that package is loaded.

The existing symbol hover (locals, document defs, prelude/builtins — incl. real
docstrings for project-defined functions, since a `defn`'s leading string is
retained on the closure and `(doc fn)` returns it) was already complete and is
unchanged. `docs/lsp.md` updated. 8 new tests (module_ref ×6, definition ×2 for
`:use`/`:implements`, hover ×2); `cargo test -p brood-lsp` green (95).

## 2026-06-14 — LSP document links + variadic-callback arity check; verified defdyn isn't statically pinned

A review-driven follow-up to the `defmodule`-clause hover/goto work. Three items
off a type-system + LSP review:

1. **Verified (no work): redefinable globals aren't statically pinned.** A review
   flagged a possible gap where the checker might pin a `def`/`defdyn` global's
   initial signature and then mis-warn after a hot-reload redefinition. Confirmed
   empirically it doesn't: `crates/lisp/src/types/check/ctx.rs:148` keeps globals
   out of the local type table (they're `dynamic()`), and a redefined global with
   no `(sig …)` produces zero warnings. Only an explicit user-written `(sig …)` is
   enforced — opt-in, and correct per `docs/types.md`. No change needed.

2. **LSP document links** (`textDocument/documentLink`). New
   `crates/lsp/src/document_link.rs`: underlines every module name in a load
   position — `(require 'foo)` args and `(:use foo)`/`(:alias foo)` clauses — with
   the resolved `foo.blsp` URI for Ctrl-click. Same `introspect::module_file`
   resolution as require-target goto; the passive whole-file counterpart to the
   cursor-driven goto. Advertised `document_link_provider` (no resolve step). 4 tests.

3. **Variadic-callback arity check** (`crates/lisp/src/types/check/walk.rs`). The
   ADR-078 callback-arity check already flagged a fixed-arity callback whose arity
   can't match a HOF's call (`(map cons …)`), and already handled *named* variadic
   globals via `arity_of`. But `lambda_literal_arity` bailed on **every** inline
   `&`/`&optional` lambda, missing the real error: a variadic lambda whose
   *minimum* arity exceeds what the HOF supplies — `(map (fn (a b & c) …) xs)`
   needs ≥2 but `map` calls with 1. Rewrote it as a phase machine returning
   `at_least(req)` for `&`, `range(req, req+opt)` for `&optional`, `exact` otherwise
   — mirroring what `arity_of` computes for named globals. Stays false-positive-free:
   `(fn (& xs) …)` (min 0) still isn't flagged. 5 catalog cases (2 warn, 3 silent).

All green: `brood-lsp` 99 tests, `brood --lib` 271, `type_check_catalog` +
`check_string_structured`. `docs/lsp.md` updated for both LSP additions.

## 2026-06-14 — JIT: lower `and`/`or` (mandelbrot ~5.3×) + fix two promotion-exposed regressions

Continuing the benchmark audit. `mandelbrot` (the suite's worst row, 1326 ms) tiered
*then deopted* on every native entry — confirmed `jit_native=0, jit_deopt=109`. Its
`esc` escape test is `(and (<= (+ xx yy) 4.0) (< i maxi))`; isolating showed `and`/`or`
itself bailed the arm, even all-integer.

- **Lower `and`/`or`** (`30156ad`). `(and X Y)` expands to `(let (g X) (if g Y g))`, so a
  comparison result crosses a block-param merge. Two bugs kept it (and *any* arm using
  `and`/`or`) off the native path: (1) block params are `I64` but a comparison is an
  `i8` (the depth interp assumed an `i8` never crosses a boundary — it does, via
  `and`/`or`); passing it raw was an `I8`-into-`I64` verifier mismatch → the whole arm
  bailed at `define_function`. Fixed by zero-extending an `i8` block arg. (2) The merge's
  else edge returns the bound slot `g` (a `Value::Bool`); `as_int` deopted on it, and
  `is_bool_op(Op::Slot)` was false, so the merge param was tagged `Op::Int` on one edge
  and `Op::Bool` on the other — last-writer-wins made a `0` (false) read as a *truthy*
  integer, looping forever. Fixed by tracking `slot_bool` (mirror of `slot_float`).
  Result: **mandelbrot 1326 → 250 ms (~5.3×)**, the suite's biggest single win — it now
  beats Elixir and Ruby. (Float *comparisons* already had codegen; this unlocked the
  control flow around them.) Verified: full in-language suite (2091) green under jit incl
  the format tiering-corruption canary; `and`/`or` parity incl non-bool returns matches
  the tree-walker.

The top-level-lambda promotion (`dfa4f67`) then turned out to have exposed **two latent
closure-serialisation gaps** (it VM-compiles closures the tree-walker used to handle):

- **RUNTIME source positions** (`8b79069`). `form-pos`/`set_form_pos` were LOCAL-only, so
  a body frozen into RUNTIME lost its position — already true for any `defn` body, and
  promotion extended it to inline lambdas, regressing `source_positions_survive_a_cross_node_send`
  (the shipped closure's quoted literal had no position → `form-pos` on the receiver
  returned nil). Added a RUNTIME position table (`RuntimeCode.positions`, the shared-region
  counterpart of the per-heap LOCAL map); `promote_list` carries each pair's position
  across. Fixes `form-pos` for both inline lambdas *and* `defn` bodies (the latter never
  worked over the wire).
- **`def`-RHS-in-`let` capture** (`84c70e7`). `(let (me 42) (def f (fn () me)))` then
  `(f)` returned "unbound symbol: me" — and the same shape broke `remote_spawn_sync`
  (shipped thunk's captured local lost → remote child crashed). `def` evaluates its RHS
  via `compile::run(heap, rhs, env)`, which built a *fresh empty Scope*; with a non-global
  `env` (a `def` inside a `let`) the enclosing lexicals weren't compile-time visible, so a
  VM-compiled capturing closure never snapshotted them. Fixed by seeding `compile::run`'s
  scope from a non-global env's lexical frames. No-op for top-level forms (`env == global`).

- **Deterministic preemption test** (`347d790`). The cpu-bound-starvation test bounded a
  liveness property with a wall-clock `receive` timeout — intrinsically flaky (false-fires
  under OS CPU starvation). Verified the scheduler is correct first (FIFO run queue,
  preempted process re-enqueues at the back), then replaced the timeout with a
  reduction-bounded drive: test-only hooks (`set_test_no_workers` + `test_drive_quanta`)
  run real scheduling quanta synchronously, so the bound is in *work units, not time*.
  Deterministic (0/40 idle, 0/20 under load) and discriminating (1 quantum → `:starved`).

Whole workspace **594/0**. Benchmark docs + positioning diagram refreshed (geomean
~16× → ~13.5× as `mandelbrot` came off the bottom; `matmul` ~39× is now the largest gap).
## 2026-06-14 — LSP: selection range, context-aware module completion, two more code actions (+ a doc-link bug fix)

A second review pass (four parallel evidence-based audits over the type checker
and LSP) found the checker "remarkably complete" — its only gaps (exhaustiveness,
redundant-clause, exception typing, `binding`-scope) are all deliberate and align
with "never reject a runnable program". It also caught a real bug in the just-added
`document_link.rs`: `collect_module_names` recursed into `Quote`/`Quasi` nodes, so a
`(require 'foo)` written as *data* (`'(require 'foo)`) emitted a spurious link —
fixed (stop descending into quoted forms) + regression test. (A claimed
`lambda_literal_arity` bug was a false alarm — the phase machine rejects a repeated
`&optional` correctly; the auditor miscalculated a boolean.)

Then three LSP additions off the review's "genuinely missing" list:

1. **Two more code actions** (`code_actions.rs`), off an `unbound symbol: foo`
   finding: **"Add `(require 'mod)`"** when `foo` is a qualified `mod/x` whose
   module resolves on the load-path (insert under any `defmodule` header, else at
   top), and **"Create function `foo`"** when `foo` is a call head — a stub
   `(defn foo (a b …) nil)` at EOF with arity matched to the call site. `quickfix`
   gained an explicit `preferred` flag so these attach the diagnostic without
   stealing the preferred slot from did-you-mean.

2. **Context-aware completion** (`completion.rs`): inside `(require '…)` or a
   `(:use …)`/`(:alias …)` clause, offer requireable **module names** alone (new
   `introspect::loadable_modules` — loaded `*features*` + top-level `<name>.blsp`
   across `*load-path*`), suppressing the generic globals that are noise there.

3. **Selection range** (`selection_range.rs`, `textDocument/selectionRange`):
   smart expand/shrink along the CST node chain — symbol → form → outer form → …
   → file — skipping trivia and same-extent wrappers. Pure tree geometry.

`docs/lsp.md` updated (capability summary, handler list, roadmap table). Tests:
`brood-lsp` 113 (code_actions ×6 new, completion ×2, selection_range ×3,
document_link ×1, end-to-end selectionRange). All green.

## 2026-06-14 — fix two cross-node regressions from the inline-lambda JIT promotion (dfa4f67)

`make test` surfaced two deterministic `cli::distribution` failures. Git-bisect
(automated, isolated worktree) pinned **both** to `dfa4f67 "jit: promote top-level
inline lambdas into RUNTIME"`, which started freezing an inline `(fn …)`'s body into
the shared RUNTIME region so the enclosing form is VM-compilable. Two distinct
defects fell out:

1. **Source positions lost** (`source_positions_survive_a_cross_node_send`). The
   per-process `form_pos` map only keys LOCAL pairs; once a positioned form is
   `promote`d into RUNTIME its position vanished, so `(form-pos …)` — and a closure
   shipped to a peer — returned `nil`. Fixed in **`8b79069`**: a `RwLock<HashMap>`
   RUNTIME position table (the shared-region counterpart of the LOCAL `form_pos`
   map), carried by `promote_list` and read by `form_pos` for RUNTIME pairs — which
   also restores positions for `defn` bodies (never preserved over the wire).

2. **Captures lost** (`remote_spawn_sync_returns_a_usable_remote_pid` — the remote
   child died `unbound symbol: me`). `compile_make_closure` promoted a closure that
   captures an *enclosing* lexical, but `compile_captures` snapshots those by name
   (`Node::Global`), which resolves via the local env chain yet does NOT survive
   being shipped to another node. Fixed at the root in **`84c70e7`**: actually
   capture enclosing lexicals when compiling a `def` RHS inside a `let` (so the
   capturing closure keeps its VM-compiled fast path *and* ships correctly).

(A parallel local fix was written from the same bisect — a `CodeSlabs.pair_pos`
table for #1 and a "don't promote capturing lambdas" gate for #2 — but the above
commits landed on `main` first and are equivalent/better, so the local fix was
dropped on rebase.) Full suite green (609). The async `remote-spawn` already worked;
only the capturing + promoted path was broken, which is why the regression hid
behind the sync variant and the position-reflection test.

## 2026-06-14 — atomic spawn-link: a real supervisor bug behind a flaky test

Chasing a flaky test (`supervisor: a transient child is NOT restarted after a clean
:normal exit`, intermittent under load) turned up a genuine concurrency bug. Confirmed
it standalone in isolation (so not cross-test leakage): under 12-core load, **17/300**
runs spuriously restarted a `:transient` child that exited `:normal`. Adding a 30 ms
delay before the exit made it 300/300 clean — so the trigger is a child exiting
*during/just-after startup*.

Root cause: `supervisor--start-child` does `pid (start)` then `(link pid)` — a
**spawn→link gap**. A child that exits in the gap is already dead when linked, and the
kernel's `link`-on-a-dead-pid delivers `[:EXIT pid :noproc]` (the real reason is lost);
`:noproc ≠ :normal`, so `supervisor--restartable?` fires the `:transient` restart. The
`link` test had the identical race (`(spawn :ok)` then `link` → `:noproc` 3/80 under
load) and `dynamic_test` the `monitor` analogue — three symptoms of one missing kernel
primitive.

Fix — **atomic `spawn-link`** (Erlang `spawn_link`): register the parent↔child link
*before* the child is enqueued, so it can't exit before the link exists and its true
exit reason is always delivered.
- `scheduler.rs`: `spawn_linked` / `spawn_impl(link_parent)` calls `links::link`
  (idempotent) right after the `REGISTRY` insert, before `enqueue`.
- `builtins.rs`: `%spawn-link`; the `spawn-link` macro (prelude) now lowers to it
  (was a non-atomic spawn-then-link, with the same gap its own docstring admitted).
- `supervisor.blsp`: documents that supervised children's `:start` should `spawn-link`
  (the Erlang convention) — examples updated.

Tests hardened to the race-free pattern (and the redundant wall-clock `(after N)`
liveness guards dropped — the runner's 120 s per-test watchdog is the real hang-guard):
`supervisor_test` (per-test tagged `[:up]` streams + `spawn-link` specs), `link_test`
(the `:normal` peer waits for `:go`; the survivor child uses `spawn-link`), `dynamic_test`
(the crasher waits for `:go` so the monitor lands before it dies).

Verified: supervisor 300/300 `:none` under load; supervisor + link 0 flakes across many
suite-under-load iterations; full workspace 609/0 under normal load. **Not yet done:**
other concurrency tests still show wall-clock timing-fragility under *pathological*
12-core saturation (e.g. the nested-sub-supervisor teardown cascade) — same class, a
separate test-hardening sweep; the suite is clean under normal CI and the underlying
kernel race is gone.
## 2026-06-14 — telemetry: an Erlang-shaped `:telemetry`, inline dispatch (ADR-106)

Added `std/telemetry.blsp` (`require 'telemetry`), the instrumentation seam a web
framework (`../hatch`) and the daemon want: `emit` a named event with measurements +
metadata, `attach` handlers to it, `span` to bracket work with
`:start`/`:stop`/`:exception`. Plus a sibling roadmap item — an ETS-like in-memory
store — was written up (deferred; "with a better name" than ETS, TBD).

**The design pivot.** The first cut made `emit` an async cast to a single `proc/gen`
registry process — framed as "improved over Erlang's synchronous handlers." On
review (the user asked *why* Erlang blocks) that's backwards: Erlang's `:telemetry`
runs handlers **inline in the calling process** off an ETS table precisely so there's
**no process in the dispatch path** — concurrent across callers, zero-copy, no
bottleneck. A single async registry instead funnels every event on the node through
one mailbox (a throughput ceiling + per-event heap copy) — worse for a web server
that runs a process per request. So we **matched Erlang's model**:

- Dispatch is **inline** in the emitting process, reading `*telemetry-handlers*`, a
  **`def`-rebound global** map of event → handlers. `def` is visible across processes
  on next lookup (ADR-013), so a handler attached anywhere is seen by an `emit`
  everywhere — the cross-process sharing Erlang gets from ETS, via Brood's one
  mutation. The hot path only *reads* the global.
- **Async is opt-in, per handler:** `forward(id, event, pid)` attaches a handler that
  only `send`s to a process you own — heavy work runs there, off the hot path.
- A throwing handler is **caught and detached** ("unhook if it fails"): `emit`'s
  dispatch collects the ids that threw and rebinds the table without them.
- Events are plain values matched by `=` (`:request` or `[:http :request :stop]`);
  `span` appends `:start`/`:stop`/`:exception` to a base vector. No-op when nothing's
  attached.

attach/detach mutate a global, so they're configuration-time (not concurrency-safe
against each other) — same as Erlang, and the deferred ETS-like store would make them
atomic later (a clean swap, since dispatch already reads a shared table).

`tests/telemetry_test.blsp` — 11 cases (emit/filter/attach/detach/re-attach,
crash-detach, the span lifecycle, no-handlers no-op, and a cross-process block where
the handler runs in each of 40 emitting processes). Registered in `builtins.rs`; docs
in `language.md` + ADR-106. Dropping the registry process also dropped the test peak
from ~34 MB to ~9.5 MB. Next: wire it into hatch's request lifecycle; then the
ETS-like store.

## 2026-06-14 — `table`: an in-memory shared store (Brood's ETS, ADR-107)

Added `table` — shared, concurrently-mutable key→value state, the escape hatch for
state many processes read/write directly without a per-owner mailbox round-trip
(Erlang's ETS). Surface (deliberately small, "fewer features but more robust"):
`table` / `table-put` / `table-get` (+default) / `table-has?` / `table-delete` /
`table-incr` / `table-count` / `table-snapshot` / `table-drop` + the `table?`
predicate.

**Representation.** A new `Value::Table(u64)` + `Tag::Table` — a scalar handle into a
global registry (`crate::table`) of `Arc<Store>`. Unlike `Socket`/`Subprocess`
(process-local) it is **sendable** (`Message::Table`): every copy of the handle
indexes the same store, the way a `Pid` names one shared process. Extending the value
universe touched the usual compatibility-contract sites (`value.rs` tag/name/keyword
arrays, `heap.rs` `tag_rank`/`hash_value`/`equal`, `printer.rs`, `message.rs` to/from,
`dist/wire.rs` reject, `types/mod.rs` ALL_TAGS + `table?`, `annot.rs`, and one
exhaustive match in `nest/mcp.rs`).

**Why it can't corrupt (the design crux).** The store holds **deep clones in
`Message` form** — the same heap-independent serialization a cross-process send uses.
Nothing in it is a live GC handle, so the moving collector never traces/moves/dangles
into it (the whole use-after-GC class is structurally excluded), and `table-get`
reconstructs a *fresh* value in the caller's heap (ETS copy-in/copy-out — no
cross-process aliasing). Key equality is **borrowed from the heap**: bucket by
`hash_value`, resolve the (rare) collision by reconstructing the stored key and
calling `Heap::equal` — so table keys behave exactly like map keys, no parallel
equality code. Locking is flat (registry `Mutex` → clone `Arc` out → drop → store
`Mutex`; never nested).

**The two things that matter.** `table-incr` is the one atomic mutator — a lock-held
read-modify-write, so concurrent counters never lose an update (no closure-based
`update`: arbitrary code under the lock can't be made safely atomic). `table-snapshot`
returns a consistent point-in-time immutable map (the MVCC win over ETS's dirty
reads), and doubles as the enumeration primitive (`keys`/`vals`/`reduce` over it).

This started as the structure question for telemetry: telemetry's handler table rode a
`def`-rebound global (ADR-106) — fine for a tiny startup table, but not atomic and it
churns the *code* region. `table` is the right structure for real shared data, and the
user's instinct ("clone it") is exactly the safety mechanism. Deferred (ADR-011):
owner-death GC / `heir`, ordered/bag tables, select/match, the distributed tier.

**Review pass.** Three independent adversarial reviewers (concurrency + GC-safety;
correctness + message round-trip; Value/Tag-integration completeness) found no crash,
corruption, deadlock, mutex-poisoning, GC-safety, or leak defect — the GC-safety
argument (handles are append-only slab indices; GC fires only at the eval safepoint,
never mid-builtin; the store holds only owned `Message` clones) and the `table-incr`
atomicity (one continuous lock hold over read+write) both hold. Two real edge-case
footguns were surfaced and fixed: a **NaN key** (NaN ≠ NaN → unretrievable + leak on
repeat put) and a **closure key** (identity not preserved across the clone) are now
rejected by `check_key`; `table-incr` on an out-of-i64 (bignum) value gives a precise
error instead of a misleading "not an integer". `tests/table_test.blsp` grew to **35
cases** (put/get/default, stored-nil-vs-absent, structural + handle + rejected keys,
has?/delete/count, atomic incr, snapshot consistency, identity, drop + use-after-drop
on every op, value round-trip incl. callable closures, 500-entry scale, and
concurrency: shared handle, explicit-message handoff, concurrent incr on one and
several keys with no lost update); green under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`.
Full suite green (608). Docs: `language.md` (new "In-memory tables" section), ADR-107,
roadmap ticked.

## 2026-06-14 — telemetry: reverse to a listener process so a handler can never crash the emitter (ADR-106)

Hard requirement from the user: **telemetry must never be able to crash the process
that emits an event — only a dedicated listener may crash.** The previous cut was
*inline* dispatch (handlers run in the emitting process, throwing handlers caught via
try/catch). That can't meet the requirement: an *uncatchable* handler fault — a
coroutine stack overflow (uncatchable segfault) or `(exit … :kill)` (untrappable) —
runs in the caller and takes it down. `try` can't wrap that. The only fix is to run
handlers in a different process.

Rewrote `std/telemetry.blsp` to a **listener model**: `emit` is a fire-and-forget
`send` to a listener process; handlers run there. So `emit` only builds the payload
and sends (a send never throws) — no handler code touches the caller, full stop. The
listener runs each handler under try/catch (a throwing handler is caught + detached,
so the listener survives normal bugs); only an uncatchable fault kills the listener.
The handler table stays a `def`-rebound global (not listener state), so the listener
is a stateless executor — supervise it and a crash restarts it with handlers intact.
Added `start-telemetry`/`stop-telemetry` (spawn/stop + register `:telemetry`) and
`telemetry-sync` (a FIFO round-trip to flush, for tests/shutdown; times out rather
than hanging). `emit`/`attach`/`detach`/`detach-all`/`forward`/`handlers`/`span`
unchanged in spirit.

The trade-off (one listener = a serialization point + a cross-heap copy per event) is
accepted deliberately: the requirement is safety, not throughput. Handlers are meant
to be cheap (log a line, bump a `table` counter) or `forward` heavy work to their own
process. This is the third and final dispatch shape this session (async-registry →
inline → listener); inline was right for throughput but wrong for the crash-isolation
guarantee, which wins.

`tests/telemetry_test.blsp` grew 11 → **19 cases**, including the headline guarantee: a
handler that does `(exit (self) :kill)` kills the listener (confirmed via a monitor
`[:down]`) while the emitting process keeps computing; plus recovery-after-crash
(restart, handlers survive in the global), "handler's `(self)` is the listener not the
emitter," throwing-handler caught+detached+listener-survives, forward, span
start/stop/exception, not-started/after-stop no-ops, telemetry-sync ordering, and a
200-process concurrent emit. Green normally and under `BROOD_GC_STRESS=1
BROOD_GC_VERIFY=1`. Updated the hatch request hook's comment (emit is now a
non-blocking send — a slow handler no longer slows the worker). Full suite green
(608). Docs: ADR-106 rewritten, `language.md` Telemetry section, roadmap.

## 2026-06-14 — `lambda`/`let*` are real synonyms; three checker false-positives fixed

Started as a type-system review and turned up a genuine bug: `lambda` and `let*`
were **documented as working** (the `foreign_construct_hint` comment in `eval/mod.rs`
listed them under "those Just Work", so it deliberately withheld a "use `fn`/`let`"
hint) but were actually **unbound at runtime** — `((lambda (x) x) 5)` raised
`unbound symbol: lambda`. So a Scheme/CL user typing `lambda` got a bare unbound
error with no guidance, and the advisory checker's "unbound symbol: lambda" warning
was a *true* positive, not a false one.

Made them **exact synonyms** (the documented intent): `lambda` → `fn`, `let*` → `let`
(`let` is already sequential). Two touch points: the evaluator's `SPECIAL_SPELLINGS`
gains both (so a raw/un-expanded eval path — a quasiquote-built or `(eval '(lambda …))`
form — dispatches them), and `macroexpand_all` **canonicalises the head** right after
the quote guard (so quoted data keeps its spelling) and before lowering — so the whole
downstream pipeline (pattern lowering, the VM compile pass, the tree-walker's lowering
re-entry) only ever sees `fn`/`let`, with no scattered `kw::FN`/`kw::LET` edits. Both
get full parity: destructuring params, variadic, multi-arity, recursion, and closures
that round-trip across processes (new `tests/lambda_let_star_test.blsp`, incl. an
`:isolated` cross-process block). Added to the LSP-facing `SPECIAL_FORMS` list too.

While proving fn-parity, surfaced and fixed **three pre-existing checker
false-positives** (all fired identically for `fn`/`let`, so not lambda-specific):
1. **`lambda`/`let*` flagged unbound** in whole-file mode — they were missing from the
   checker's `SPECIAL_HEAD` / `is_syntactic_keyword`. A new `is_fn_head` helper unifies
   the `fn`/`lambda` recognition so the callback-arity and return-type checks see through
   `lambda` too.
2. **Multi-arity fn clause params** — `check_fn` read the first clause `((a) …)` as a
   param list, so a param used only in a *later* clause (`((a b) …)`) looked unbound.
   Now detects the multi-clause shape (reusing the evaluator's `fn_is_arity_multi_clause`,
   so checker and runtime agree) and binds every clause's params before walking the bodies.
3. **Self-recursive `let`-bound closure** — `(let (fac (fn (n) … (fac …))) …)` flagged
   `fac`, though it resolves at runtime (the closure captures the frame, late-binds on
   call). The checker now pre-binds fn-valued `let` names (widening scope only — an eager
   forward reference in a non-closure RHS still surfaces).

Also folded in a round of **type-system doc/cleanup** from the review: `docs/types.md`
gained the three previously-undocumented Step-4 passes (protocol/behaviour conformance,
the non-tail-recursion lint, the dead-clause lint) and an updated "Where it lives"
(the `check/` submodule split); deleted dead `ctx::resolve_param`; collapsed a
`list_result` duplication in `guards.rs`. Suite green: 113 checker unit tests, 273 lib,
2163 in-language (both engines), GC-stress clean.

Also closed a `nest run` gap: an explicit `nest run FILE.blsp` ran the file via `(load …)` without the advisory pre-check that `nest run` (:main, via `check-project-sources`) and `brood <file>` already do. `cmd_run` now pre-checks the file with `check-file` (single-file, GNU warnings to stderr, `BROOD_NO_CHECK=1` opts out) before running — so every run path checks first.

Decision recorded as ADR-108.

## 2026-06-14 — JIT matmul LICM: hoist an invariant vector's element base out of the loop

First fruit of the immutability-shortcut framing (`compute-frontier.md` §6): a tight
loop that reads an immutable vector by a varying index — `(nth rowa k)` in matmul's
`dot` — was paying a full `brood_rt_vector_ref` call per element (~9.7 ns: marshal 6
words + a slab lookup + a 24-byte out-pointer copy). Because Brood data is immutable
(ADR-026), **no write can ever invalidate that read**, so the load is loop-invariant
with *zero* alias analysis — even this template JIT can hoist it soundly.

The JIT now: (1) computes a self-recursive arm's **loop-invariant parameter slots** at
the `Node` level — slot `k` is invariant iff every `SelfCall` passes `Node::Local(k)`
unchanged; (2) for each invariant slot read as a fused `(nth slot idx)`
(`Prim2SlotSlot{VectorRef}`), resolves the vector's element `(data_ptr, len)` **once**
in the entry block via a new `brood_rt_vector_base` helper (each Brood vector's inner
storage is a contiguous `Vec<Value>`, so `ptr + idx*size_of::<Value>()` is a flat read);
(3) replaces the per-element call with an inline bounds-checked 3-word load.

**Soundness.** The hoisted raw pointer is only valid for one native run, so the hoist is
gated to arms that neither allocate (`cons`/vector-build → LOCAL GC) nor make a
Brood→Brood call (could `def` → RUNTIME compaction) — under that gate nothing relocates
the storage mid-run, and a preempt/deopt re-enters from the entry block (re-hoist). A
non-vector slot, a non-int index, or an out-of-range index all **deopt** so the VM
produces the exact result — JIT==tree-walker parity preserved. Globals are deliberately
*not* hoisted: a `def` rebind from another process between iterations would diverge from
the VM's late binding.

**Measured.** Isolated invariant-local read: ~7.8 ns → ~1.2 ns (a ~6.5× drop on the read
itself). matmul N=175: 290 ms → 250 ms (~14%; only `(nth rowa k)` is hoistable — `b` is a
global and the inner row varies per `k`). Generalizes to any indexed loop over an
invariant local vector. Verified: matmul + microbench JIT==tree-walker checksum parity,
`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` clean, full in-language suite green under
`--features jit` (format-tiering canary included), both feature builds compile.

## 2026-06-14 — Checker false-positive sweep (bucket A): transient args, unexpandable macros, dynamic-namespace refs

Three remaining advisory-checker false-positive classes, all fixed (the checker's
prime directive is zero false positives). Project-wide `nest check` over `std/` +
`tests/` dropped from 38 warnings to 3 — and those 3 are the *intentional* non-tail
recursion lint on `pattern_matching_test`'s `pm-fac`, a true positive.

1. **Transient as a `count`/`length`/`contains?` argument.** These ops dispatch to
   `transient-*` kernel hooks when handed a live transient, so a transient is a valid
   argument — but their curated sigs only admitted `str|map|seq`. Widened `countable`
   to include `Tag::Transient` and gave `contains?` a `map_or_transient` domain
   (`sigs.rs`). Domains stay otherwise tight (a number still warns).

2. **Unexpandable macro calls.** A file-local macro the checker can't expand
   (single-file mode, or one defined inside a deferred `test`/`describe` thunk) had its
   argument subtree walked as ordinary code, and a name it `def`d went unrecorded — two
   FPs: `(defmacro wp (v & body) ` `` `(let ((a b) ~v) ~@body)) ``  then `(wp x (+ a b))`
   flagged `a`/`b` (opaque syntax `wp` splices into a binder), and `(mk qf)` (where `mk`
   `def`s `qf`) left a later `(qf 5)` flagging `qf`. Fix: track file-local macro names
   (`Ctx.file_macros`), detecting the lowered `(def name (%make-macro …))` shape since
   `defmacro` is gone post-expansion; the walk skips descending into such a call's args,
   and `collect_def_names` absorbs its bare-symbol args as possible globals (sound —
   only widens the bound set). A genuine unbound head under a known callee still warns.

3. **Qualified references to a dynamically-defined namespace.** `namespace_test` /
   `package_test` build modules inside `%load-string` strings or temp files loaded at
   runtime, so `(nsA/greet)` / `(greeter/greet …)` reference names the checker can't see
   statically — ~25 FPs. Fix: `check_file` records the set of *known namespace prefixes*
   (every `mod/` for which some `mod/<name>` global is loaded), and the unbound check
   stays silent on a qualified name whose module isn't among them — we can't prove it
   unbound. A typo in a *known* module (`(test/no-such-fn …)`) is still flagged, so the
   useful qualified-typo catch is preserved. (`Arc<HashSet<String>>` in `Ctx` keeps the
   per-scope clones cheap.)

Regression tests for each in `check.rs`; 116 checker unit tests, 278 lib, 2163
in-language — green. (Bucket B/C — deeper inference, gradual-assignment checking — stay
deferred per ADR-011 until a real consumer needs them.)

- **2026-06-14** — `string-split` made a **native builtin** (ADR-109): the pure-Brood
  version re-`substring`ed the tail each step and char-indexed `substring` is O(index), so
  splitting was O(n²) — a 174 KB `git ls-files` parse took ~840 ms in brood-edit's
  project-file scan, now ~10 ms (one `str::split` pass). Removed `string-split`/
  `string-split--acc` from `std/prelude.blsp`; semantics unchanged (empty sep → chars), so
  `tests/strings_test.blsp` and the ~10 std modules built on split (file/path/text/diff/
  datetime/url/http/sse) are unaffected. 2150 in-language tests green.

## 2026-06-14 — Structured types, fifth slice: element flow through the rest of the sequence library

Bucket B, the additive/low-risk slice (chosen over speculative body inference, which
ADR-011 defers and which is the historical false-positive source). Extended the
checker's element-type flow (`seq_aware_call_ty`) from the dozen combinators it
already handled to the rest of the element-preserving / -extracting sequence library:
`second`/`third` (extract, `A | nil`), `rest`/`but-last`/`distinct`/`dedupe`/
`take-last`/`drop-last`/`remove` (preserve, `nil | list<A>`), `keep` (map-then-drop-nil,
`nil | list<B>`), `interpose` (`nil | list<A | type(sep)>`), and `range`
(`nil | list<number>`).

So `(+ 1 (first (rest ["a" "b"])))` and `(string-length (first (range 5)))` are now
caught, where before the result fell back to a flat `list`. Soundness is structural:
each rule produces the *exact* element type (preserve/extract) or a sound superset
(`keep`'s callback return keeps `nil`; `range`'s `number` covers int/float; `interpose`
unions the separator), and `is_disjoint` still decides on tags alone and never inspects
an element refinement — so a refinement can only *sharpen* a downstream result, never
manufacture a false positive. Project-wide `nest check` over `std/` + `tests/` stays at
3 warnings (all the intentional non-tail recursion lint). 117 checker unit tests, 279
lib, 2163 in-language — green.

Bucket B's other slices (sound branchy-body inference; wiring up the unconsumed
`GradualTy` for gradual-assignment checking) remain deferred per ADR-011 until a
concrete consumer needs them.

## 2026-06-15 — GradualTy gets its first consumer: gradual-assignment checking of `(def x …)` vs `(sig x T)`

`GradualTy`/`consistent_with` were built-and-tested but **unconsumed** — referenced
only by their own unit tests, with a standing "wire it in only when a real
gradual-assignment consumer arrives" note. Rather than delete the island (the
greenfield instinct), gave it the consumer the note asked for.

A non-arrow `(sig x T)` declares a **value type** (previously grammar-accepted but
dropped by the checker). The new check: `(def x <expr>)` must assign a value
*consistent* with `T`. The design is what makes `GradualTy` actually earn its place
over the existing `Option<Ty>` disjointness pass — **assignment uses consistent
subtyping, not disjointness**:

- A reference to a redefinable global with a declared type is `dynamic_within(t)` —
  a *bounded dynamic* that `Option<Ty>` (only known/unknown) structurally can't
  represent. So `(def count label)` with `label : string`, `count : int` is flagged
  (`string ∩ int = ⊥`), where the disjointness pass — treating every global as an
  untracked `None` — sees nothing.
- A precise **literal** is `stat(t)` (checked with `⊆`): `(def n "hello")` against
  `(sig n int)` flags.
- An **over-approximated** value (a call result, a local) is `dynamic_within(t)`, so
  consistency uses `∩ ≠ ⊥` and can't over-warn on a widened guess: `(def n (+ 1 2))`
  against `int` *defers* (`number ∩ int ≠ ⊥`), not warns — no false positive. An
  unknown global → pure `dynamic()` → always defers (hot-reload safe).

Implementation: `annot::parse_value_sig_decl` (non-arrow sigs), `Ctx.declared_value_ty`,
`walk::gradual_of` (the expression → `GradualTy` bridge) + the check in `check_def`.
Project-wide `nest check` over `std/` + `tests/` stays at 3 (the intentional recursion
lint) — zero new false positives. 120 checker unit tests, 279 lib, 2163 in-language —
green. Updated the now-stale "unconsumed island" status notes in types.md /
type-annotations.md / the check.rs module doc.

This is the first slice of bucket B's gradual-typing work; return-type and
declared-param assignment checks remain deferred (the return check needs the
sound body inference ADR-011 still defers).

## 2026-06-15 — Gradual typing, slice 2: return-type checking + declared globals in value position

Two more `GradualTy`-backed checks on top of the `(def x …)` assignment check, both
FP-clean across `std/` + `tests/` (project-wide `nest check` stays at 3 — the
intentional recursion lint).

**Return-type checking.** A `(sig f (P… -> R))` now also checks that the body's last
form yields a value *consistent* with `R`. Reuses `gradual_of`, so the soundness is the
same: an over-approximated body (a call) is `dynamic_within(t)` and the `∩` relation
only warns on a body type provably disjoint from `R` — `(sig f (int -> string))` with
body `(+ x 1)` flags (`number ∩ string = ⊥`), while `(sig inc (int -> int))` with the
same body *defers* (`number ∩ int ≠ ⊥`), no false positive. A precise literal body uses
`⊆`. Threaded the function name through `check_fn_seeded` for the diagnostic
(`f: declared return type string but the body yields number`). Single-clause sig'd fns
only (multi-arity return checking deferred).

**Declared globals in value position.** `expr_ty` for a bare global now falls back to a
`(sig g T)` value-type declaration, so `g`'s declared type flows into the disjointness
check: `(string-length g)` with `(sig g int)` is flagged, and it threads through a `let`
binding too. A redefinable global is still only ever warned on a *provable* mismatch
with its declared (contract) type and deferred on overlap — exactly `dynamic(T)`'s
behaviour, honouring contract #4; a lexical local shadows it.

`walk::gradual_of` is now the shared expression→`GradualTy` bridge for all three
gradual checks. 123 checker unit tests, 282 lib, 2163 in-language — green. Still deferred
(ADR-011): branchy-body inference for *precise* return types (today's return check is
disjointness-sound but can't catch a body merely *wider* than R), and gradual
intersection/negation.
