# Brood documentation

This folder is the detailed record of what Brood is, how it's built, and where
it's going. Start here.

| Document | What's in it |
|---|---|
| [architecture.md](architecture.md) | The big picture: the runtime, the crate layout, the eval loop, the memory model, and the "one runtime that can also be a server" design that the whole project is organised around. |
| [components.md](components.md) | The **component map**: every module/crate/std file, what it owns, its interface (its *seam*), its dependencies, and what's safe to work on independently — plus a separation-of-concerns assessment. The "who does what" companion to architecture.md's "why". |
| [spec.md](spec.md) | The **formal language specification** (v0.1): lexical structure and reader grammar (EBNF), the data model, evaluation/tail-call rules, scoping (it's a Lisp-1), special forms, and the primitive/derived split. The precise companion to language.md. |
| [language.md](language.md) | The language reference *as implemented today* (v0.1): data types, syntax, special forms, and every builtin. Friendlier than the spec. |
| [primitives.md](primitives.md) | The **native primitive kernel** — the complete list of functions implemented in Rust (everything else is Brood), including how error handling (`throw`/`%try`/`try`/`error`) is built. |
| [concurrency.md](concurrency.md) | Design (for review) for **green processes on all cores** — Erlang-style `spawn`/`send`/`receive`, share-nothing, work-stealing schedulers. A parallel core track. |
| [memory-model.md](memory-model.md) | Design (for review) for **`Send` heaps + GC** — the prerequisite for true multi-core. Compares gc-arena+stepping-VM vs a hand-rolled arena; staged migration plan. |
| [shared-code.md](shared-code.md) | **Shared code, isolated data** (Erlang-style, implemented) — region-tagged handles, a runtime's mutable shared code region + global table, and **cross-process hot reload** (a `def` reaches running spawned processes). Cheap spawn; separate runtimes stay independent. |
| [testing.md](testing.md) | The **test framework** (`std/test.blsp`, written in Brood): ExUnit / `mix test`-style `describe`/`test`, the assertions, the **parallel-by-default** execution model with `:serial`/`:isolated`, and how share-safe tallying works. |
| [packages.md](packages.md) | Design (not yet implemented) for **third-party Brood deps** — Git URLs as package identity, project-local `_deps/` cache, `project.lock.blsp` for reproducibility, no central registry. Manifest model, resolution algorithm, conflict policy, and the security argument (no install scripts; supply-chain class closed by construction). ADR-037. |
| [supervision.md](supervision.md) | Design (not yet implemented) for **supervised-by-default processes with mode-gated resume checkpoints** — a process *is* its current call, captured at every iteration boundary; an uncaught error triggers automatic resume from the last good state, same args, new code (after a reload). Mode gate: full supervision in dev (paid per call), restart-only in release (zero per-call cost). When this lands: `defonce` (currently in the prelude as a transitional shim) is removed in the same commit that introduces named-spawn; `live-loop` never needs to exist; `std/reload.blsp`/`std/hatch.blsp`/`nest test` simplify. ADR-039. |
| [roadmap.md](roadmap.md) | The milestones (M1 → M5), what's done, and what's next — including the editor, the display protocol, and the remote/web frontends. |
| [decisions.md](decisions.md) | The design-decision log (ADR-style): the *why* behind the choices, so future-us doesn't relitigate them by accident. |
| [devlog.md](devlog.md) | A chronological log of work sessions — what changed and why, in order. |
| [claude-demo-findings.md](claude-demo-findings.md) | An LLM's notes after writing a concurrent Mandelbrot demo in Brood from scratch: language familiarity gaps, the multi-thread scheduler race, interpreter perf numbers, tooling friction (formatter, type-checker noise), and a prioritised fix list. |
| [llm-native.md](llm-native.md) | Forward-looking companion to claude-demo-findings.md: what would make Brood a language LLMs genuinely write well in — an MCP server, a shipped Claude skill, structured errors with codes, macroexpand-on-everything, a watch-mode REPL, an example-by-intent index, idiom-aware lints, property tests, the "Brood gauntlet" eval harness, and a self-improving "incarnations" file. |

## The one-paragraph version

Brood is a small, dynamic Lisp implemented in Rust. Its reason for existing is
to be the language a modern, Emacs-like text editor is *written in* — so that a
running editor can redefine its own behaviour by re-evaluating code. v0.1 is the
language core: a reader, a tree-walking evaluator with proper tail calls and
lexical closures, a set of builtins, and a REPL. The editor, the display
protocol, and the remote/web frontends come later (see the roadmap).
