# Brood documentation

This folder is the detailed record of what Brood is, how it's built, and where
it's going. **Start with `architecture.md` and `components.md`**, then dive into
the area you need below. (For working *in* the repo, the canonical guide is
[`../CLAUDE.md`](../CLAUDE.md); for writing Brood *code*, [`brood-for-claude.md`](brood-for-claude.md).)

> Every `.md` in this folder is indexed here. If you add a doc, add a row — the
> index being complete is what lets a reader (human or LLM) trust it over `ls`.

## The one-paragraph version

Brood is a small, dynamic Lisp implemented in Rust. Its reason for existing is
to be the language a modern, Emacs-like text editor is *written in* — so that a
running editor can redefine its own behaviour by re-evaluating code. v0.1 is the
language core: a reader, an evaluator (now a closure-compiling VM) with proper
tail calls and lexical closures, a Brood-written standard library, the
Erlang-style concurrency/distribution runtime, and the first vertical slices of
the editor (a rope/buffer model and a display protocol). The editor app, the
server, and the web frontend come later (see the roadmap).

---

## Start here — orientation

| Document | What's in it |
|---|---|
| [architecture.md](architecture.md) | The big picture: the runtime, the crate layout, the eval loop, the memory model, and the "one runtime that can also be a server" design the project is organised around. |
| [components.md](components.md) | The **component map**: every module/crate/std file, what it owns, its interface (its *seam*), its dependencies, and what's safe to work on independently. The "who does what" companion to architecture.md's "why". |

## Language reference (Brood as implemented)

| Document | What's in it |
|---|---|
| [language.md](language.md) | The language reference *as implemented today* (v0.1): data types, syntax, special forms, every builtin. Friendlier than the spec. |
| [spec.md](spec.md) | The **formal specification** (v0.1): reader grammar (EBNF), data model, evaluation/tail-call rules, scoping (Lisp-1), special forms, the primitive/derived split. The precise companion to language.md. |
| [primitives.md](primitives.md) | The **native primitive kernel** — the complete list of functions implemented in Rust (everything else is Brood), including how `throw`/`%try`/`try`/`error` are built. |
| [pattern-matching.md](pattern-matching.md) | `match` + destructuring in `let`/`fn`: the pattern language and the one Brood compiler reused at every binding site (ADR-021). |
| [namespaces.md](namespaces.md) | Modules & namespaces: `defmodule`, `require`, `(:use …)`, expand-time resolution over the flat table, soft privacy, collision policy (ADR-065/070/085). |
| [types.md](types.md) | The **set-theoretic, gradual, advisory** type system: the `Ty`/`GradualTy` lattice, subtyping as set inclusion, and the **compatibility contract** to check before adding a `Value`/primitive/form (ADR-023/024/078). |
| [type-annotations.md](type-annotations.md) | Opt-in `(sig …)` / `(sig! …)` annotations & runtime contracts (ADR-082). |
| [parametric-result-types.md](parametric-result-types.md) | How element types flow through parametric HOFs (`map`/`filter`/`reduce`), ADR-078. |
| [error-codes.md](error-codes.md) | The `E00xx` error-code catalogue and the "errors that teach" philosophy. |
| [interop.md](interop.md) | Foreign-function / native-extension design: WASM components built on fetch, wrapped in Brood (ADR-071, proposed). |

## Runtime & internals

| Document | What's in it |
|---|---|
| [memory-model.md](memory-model.md) | `Send` heaps + GC — the prerequisite for true multi-core. gc-arena vs hand-rolled arena; the staged migration. |
| [shared-code.md](shared-code.md) | **Shared code, isolated data** (implemented): region-tagged handles, a runtime's mutable shared code region + global table, and cross-process hot reload (a `def` reaches running spawned processes). |
| [bytecode-vm.md](bytecode-vm.md) | The **closure-compiling VM** (now the default engine, ADR-076): compile model, stages, closure capture, source positions. |
| [vm-perf-and-jit-runway.md](vm-perf-and-jit-runway.md) | VM-interpreter perf round + the JIT runway (ADR-096): VM-vs-JIT framing, the IC/rooting/prim work list, JIT-alignment rules, benchmark protocol. |
| [compute-frontier.md](compute-frontier.md) | **Post-JIT compute roadmap** (2026-06-14, planning): the easy codegen wins are done (geomean 19.5→13.5×); the remaining single-threaded gaps profile as *data-structure*-specific (flat vectors for `matmul`, lazy combinators for `strings`/`pipeline`, allocation for `bintree`) — not `Value`-width. Scoped levers + how to pick up. |
| [benchmarking.md](benchmarking.md) | **How to benchmark & profile the VM**: the load-robust VM÷tree-walker ratio (`scripts/bench-ratio.sh`) for *timing*, the `perf-stats` counters + `(vm-stats)` for *attribution*, and how the latter feeds the bytecode-lowering gate. |
| [transients.md](transients.md) | Internal transients — fast bulk building of immutable maps/vectors without exposing mutation. |
| [live-editing.md](live-editing.md) | Hot reload & live redefinition: `def` semantics across processes, `defonce`, reload detection, macro-staleness (ADR-042). |
| [scheduler.md](scheduler.md) | The green M:N scheduler: stackful coroutines, reduction-counted preemption, work distribution (ADR-018/027). |
| [concurrency.md](concurrency.md) | Original design for **green processes on all cores** — `spawn`/`send`/`receive`, share-nothing, work-stealing. |
| [concurrency-v2.md](concurrency-v2.md) | The revised concurrency track: load-balancing, the userland-supervisor direction, what changed from v1. |
| [distribution.md](distribution.md) | **Distributed nodes**: node-tagged pids, TCP links, location-transparent `send`, remote monitors, closure shipping, the HMAC handshake (ADR-034/073/074/081). |
| [node-connect.md](node-connect.md) | Node-connect ergonomics: default-cookie file, name-addressed transport, `nest run --name` (ADR-068). |
| [supervision.md](supervision.md) | **Reverted-then-userland.** The kernel supervisor that shipped briefly and was stripped (race source), why, and the userland `spawn`+`monitor` pattern that replaced it (ADR-039 reverted → ADR-044). |
| [known-issues.md](known-issues.md) | The live **KI-N** issue list: open kernel/runtime bugs, their repros, and status. Read before chasing a crash. |

## Editor (M2 and beyond)

| Document | What's in it |
|---|---|
| [building-an-editor.md](building-an-editor.md) | The plan for the editor on top of the rope/buffer/display substrate — the data model → display → input arc. |
| [layers.md](layers.md) | The display **layers** model (`std/editor/layers.blsp`): how overlays/decorations stack on a buffer view. |
| [gui-font-gaps.md](gui-font-gaps.md) | Known gaps in the GUI font/`Face` handling — a running punch-list for the windowed frontend (ADR-079). |

## Tooling

| Document | What's in it |
|---|---|
| [testing.md](testing.md) | The **test framework** (`std/tool/test.blsp`): ExUnit-style `describe`/`test`, assertions, parallel-by-default with `:serial`/`:isolated`, share-safe tallying (ADR-015). |
| [tooling.md](tooling.md) | The `nest` project tool overview: `new`/`test`/`check`/`run`/`doc`/`format`/`repl`/`observe` and friends (ADR-028). |
| [lsp.md](lsp.md) | The **language server** (`brood-lsp`): Tier 0–2 features — completion, hover, goto/refs/rename, semantic tokens, code actions (ADR-025). |
| [mcp.md](mcp.md) | The **MCP server** (`nest mcp`): a per-project Model Context Protocol surface over the live image (ADR-036). |
| [packages.md](packages.md) | The **package manager**: Git URLs as identity, project-local `_deps/`, `project.lock.blsp`, no central registry, the supply-chain argument (ADR-037). |
| [release.md](release.md) | Single-binary app bundling — `nest release` (ADR-038). |

## Planning & history

| Document | What's in it |
|---|---|
| [roadmap.md](roadmap.md) | **Canonical milestone plan** (M1 → M5): what's done, what's next — the language, the editor, the display protocol, the frontends. |
| [../ROADMAP.md](../ROADMAP.md) | The **Stage-1 completeness checklist** (top-level): "is Brood a practical general-purpose Lisp yet?" — a finer-grained tick-list that feeds the milestones in roadmap.md. |
| [../todo.md](../todo.md) | A **scratch list** (top-level) of work not yet committed to. Items graduate to roadmap.md or an ADR once decided — treat as ephemeral, not authoritative. |
| [deferred.md](deferred.md) | The **holding pen**: worthwhile work intentionally *not* done yet, each with a design sketch and the trigger that should pull it back in. |
| [decisions.md](decisions.md) | The **ADR log** (Architecture Decision Records) — the *why* behind each in-force choice, so we don't relitigate settled questions. Has an ADR index at the top; superseded/reverted ADRs are archived (see below). |
| [devlog.md](devlog.md) | A chronological log of work sessions — what changed and why, in order. Holds the complete session **digest** + the latest day in full; navigation/threads header at the top. |
| [archive/devlog-archive.md](archive/devlog-archive.md) | The **full verbatim text** of older devlog sessions, rolled out of devlog.md to keep it loadable. Search a `## YYYY-MM-DD — …` header to read one in full. |
| [incarnations.md](incarnations.md) | The self-improving "incarnations" notes — lessons an agent carries forward between sessions. |

## For LLMs / agents writing Brood

| Document | What's in it |
|---|---|
| [brood-for-claude.md](brood-for-claude.md) | The **pocket reference for AI assistants**: syntax, idioms, and the patterns that aren't shared with other Lisps. Read this before writing `.blsp`. (Shipped into every `nest new` project.) |
| [writing-brood-skill.md](writing-brood-skill.md) | Source of the `writing-brood` skill — the conventions an LLM gets wrong by default. |
| [brood-debug-skill.md](brood-debug-skill.md) | Source of the `brood-debug` skill — the recovery playbook for a crashed/hung/segfaulted Brood run. |
| [llm-native.md](llm-native.md) | Forward-looking: what would make Brood a language LLMs genuinely write well in — MCP, skills, structured errors, the "Brood gauntlet" eval harness. |
| [claude-demo-findings.md](claude-demo-findings.md) | An LLM's notes after writing a concurrent demo from scratch: familiarity gaps, the scheduler-race repro, perf numbers, a prioritised fix list. |

## Point-in-time notes & handoffs

Snapshots written during a specific debugging or design session. **Not current
reference** — several are linked from source-code comments (benches, tests,
`heap.rs`, `mcp.rs`) as the design context for a particular fix, which is why
they stay in place rather than being deleted. Read for *how a problem was chased*,
not for *how things are now*.

| Document | What's in it |
|---|---|
| [handoff-blocking-io.md](handoff-blocking-io.md) | Handoff: blocking-IO → mailbox delivery (ADR-059). |
| [handoff-eval-dispatch.md](handoff-eval-dispatch.md) | Handoff: the evaluator-dispatch performance campaign. |
| [handoff-gc.md](handoff-gc.md) | Handoff: the GC bring-up. |
| [handoff-vm-gc-memory.md](handoff-vm-gc-memory.md) | Handoff: VM × GC × memory interactions. |
| [handoff-vm-callback-routing.md](handoff-vm-callback-routing.md) | Handoff: route native higher-order callbacks (`try`/`binding`/`apply`) through the VM, blocked on the `let`-self-ref `send` divergence (fix plan inside). |
| [runtime-collector-exploration.md](runtime-collector-exploration.md) | Exploration of a collector for the shared RUNTIME code region. |
| [memory-review.md](memory-review.md) | A full memory/GC review snapshot. |
| [findings-closure-promotion-overflow.md](findings-closure-promotion-overflow.md) | Findings: closure-promotion stack overflow. |
| [gol-findings-2026-05-30.md](gol-findings-2026-05-30.md) | Game-of-Life dogfooding findings (stdlib/DX gaps). |
| [gc-flush-panic-mcp-2026-05-31.md](gc-flush-panic-mcp-2026-05-31.md) | Investigation of a GC flush panic under `nest mcp` (stale-binary). |
| [lexical-addressing-gotchas.md](lexical-addressing-gotchas.md) | Gotchas hit while adding lexical addressing / VM closures. |

## Subdirectories

| Path | What's in it |
|---|---|
| [archive/](archive/) | **Archived docs** kept out of the active tree: superseded/reverted ADRs ([decisions-superseded.md](archive/decisions-superseded.md)) and the verbatim older devlog ([devlog-archive.md](archive/devlog-archive.md)). Not current reference. |
| [benchmarks/](benchmarks/) | Archived `divan` benchmark runs, one file per run with full environment metadata (written by `scripts/bench.sh`). |
| [prompts/](prompts/) | Externalized prompts (e.g. the `nest mcp` task prompt). |
| [research/](research/) | Background research notes — chiefly the Elixir set-theoretic-types papers that informed the type system. |

---

> **Bundled in the default install:** the net library (`net/tcp`/`net/http`/
> `net/sse`) and the process framework (`proc/gen` + `proc/supervisor`) ship in
> `std/`, baked into the binary (ADR-097 — batteries-included; reverses ADR-085
> Move 2). The Rust *mechanism* for sockets lives in the `brood` lib
> (`crates/lisp/src/net.rs`); the Brood *policy* is in `std/net/*`.
