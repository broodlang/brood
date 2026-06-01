# Dev log

Chronological record of work sessions. Newest at the bottom.

## How to navigate

The session history is split so this file stays loadable:
- **This file** = the **complete digest** (every session, one line, by date) plus
  the **most recent day in full** at the bottom, where new entries get appended.
- **[devlog-archive.md](devlog-archive.md)** = the **full verbatim text** of all
  older sessions.

You rarely read either top to bottom. For the *current* state of something, prefer
the topic doc (see [README.md](README.md)) or the relevant `## ADR-NNN` in
[decisions.md](decisions.md); use the log to recover the *why* and *how* of a change.
To read a session in full, find its `## YYYY-MM-DD — …` header in
[devlog-archive.md](devlog-archive.md) (or in the "Recent" section below for the
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

Every session, oldest first. Full text: [devlog-archive.md](devlog-archive.md)
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

---

## Recent — full entries

The latest day in full; older sessions' full text is in
[devlog-archive.md](devlog-archive.md). Append new sessions below (newest last).

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
