# Changelog

All notable changes to the Brood toolchain (`brood`, `nest`, `brood-lsp`) are
recorded here. Versions follow [semver](https://semver.org); the full
engineering narrative lives in [`docs/devlog.md`](docs/devlog.md).

## v0.3.5 — 2026-08-07

- **Green processes run under WebAssembly.** `spawn`/`send`/`receive` used to trap in the
  browser (the scheduler starts a pool of OS threads, which wasm can't). On `wasm32` there
  are now no worker threads: the run queue is driven cooperatively on the single thread by a
  `pump_until_quiescent` sweep, and the top-level program runs as a green process whose
  result is rendered across the process-heap boundary (`run_program_repr`) so the playground
  can show it. Everything is behind `cfg(target_arch = "wasm32")`, so the native scheduler is
  byte-for-byte unchanged. (`now_nanos` moves to `web_time::Instant` — plain
  `std::time::Instant::now()` panics on wasm.) The in-browser playground and the runnable doc
  examples can now run concurrency snippets.

## v0.3.4 — 2026-08-07

- **`doc-catalog`** — a new CORE module mapping every public builtin/prelude function to a
  functional category (Math, Strings, Filesystem, Processes, …) plus the category order and
  titles. `nest docs --all` now emits the reference **grouped by category** instead of one
  flat list, and a shipped app (hive's `/reference`) requires the same module — so the CLI
  and hosted language reference are categorised identically from one source.

## v0.3.3 — 2026-08-07

- **`nest docs`** — a new subcommand that generates a browsable HTML documentation
  site from a project's docstrings (`doc/index.html` + `doc/model.json`); `nest docs
  --all` documents the whole builtin + prelude reference (the language reference).
- **`docsite`** — a new CORE module: a pure `model -> HTML` renderer (sidebar,
  per-module sections, signatures/types/docstrings, a client-side filter) shared by
  `nest docs` and any app that hosts docs (the styles are scoped under `.docsite` so a
  `:wrap? false` fragment embeds in a host page; the host dictates light/dark, only the
  standalone page follows the OS).
- Per-module attribution in the doc model is by namespace (via `project-file-feature`,
  accounting for ADR-070 project-name rooting), not a load-order-sensitive
  `global-names` delta.

## v0.3.0 — 2026-08-06

A maintenance release: test-runner robustness and tooling, no language or
runtime behaviour changes since 0.2.0.

- **`nest update-tooling`** — a new subcommand that re-drops the AI-assistant
  files `nest new` scaffolds (the `docs/brood-for-claude.md` reference and the
  `writing-brood` skill) from the current binary, so they don't drift as the
  language evolves. Guarded against a nil project root, and works from a
  subdirectory.
- **KI-29** — a killed test binary no longer orphans its `brood` children; the
  test harness reaps the child OS-processes it spawns.
- **KI-30** — seven temp-dir prefixes that were never purged (leaking ~168 MB of
  `/tmp` per full suite run) are now cleaned up.
- **Privacy/LSP** — review follow-ups on the ADR-146 step-2 def-site privacy
  migration; `nest doc` no longer leaks private definitions.
- Docs currency pass and perf-measurement work (both `spawn-live` "next levers"
  measured and declined — the arms already reach native code).

## v0.2.0 — 2026-08-05

- **Def-site privacy migration (ADR-146 step 2)** — `defn-`/`def-` for private
  definitions; the older `--` naming convention was removed.
- **Runtime work (ADR-213/214/215)** — shared compiled code across a runtime's
  processes and related scheduler/message-path improvements.

## v0.1.0 — 2026-08-02

- First tagged release of the Brood toolchain: the language core (immutable
  Lisp, macros, pattern matching, modules, CHAMP maps), the green-process
  runtime with distribution, the closure-compiling VM + tier-1 JIT, the
  set-theoretic advisory type checker, the self-hosted REPL, `nest` project
  tooling, and the `brood-lsp` language server.
