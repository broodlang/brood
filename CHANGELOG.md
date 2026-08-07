# Changelog

All notable changes to the Brood toolchain (`brood`, `nest`, `brood-lsp`) are
recorded here. Versions follow [semver](https://semver.org); the full
engineering narrative lives in [`docs/devlog.md`](docs/devlog.md).

## v0.3.8 — 2026-08-07

Review fixes for the doc/wasm batch:

- **wasm `receive` timeouts no longer busy-spin.** `(after ms …)` woke the process but the
  gate re-checked the *real* clock (almost no time passed) and re-parked, so the pump spun at
  100% CPU for the whole real delay — freezing the browser tab. A `cfg(wasm32)` **logical
  clock** (`timer::sched_now`) now advances to the fired deadline, so the receive resolves at
  once (a 1 s timeout returns immediately). Native uses the real clock, unchanged.
- **`markdown->html` no longer runs away on an unmatched `[`** (`index-of` returns -1, not
  nil — the missing guard recursed on the same text and stack-overflowed). An unclosed ```
  fence at end-of-input is now emitted rather than dropped, and a guide link's URL is
  attribute-escaped with `javascript:` neutralised.
- **`nest doc` (Markdown) uses namespace attribution** (`project-file-feature`), like `nest
  docs`, instead of a `global-names` load-delta — which mis-credited a module already bound
  (transitively loaded, or materialised from an ADR-218 startup image).

## v0.3.7 — 2026-08-07

- **`nest doctest`** — a new subcommand that evaluates every `expr ;=> result` example in
  the project's docstrings and checks it still holds, so a documented example can't silently
  drift from the code. Prints a line per example and exits non-zero on any mismatch (CI-
  ready). Scoped to the project's own globals; `;=>` never appears in a builtin/prelude
  docstring, so nothing else is picked up.
- **Guides in `nest docs`.** A `guides/*.md` file becomes a narrative page in the generated
  site, alongside the API reference (in the sidebar and rendered from a small Markdown subset
  — ATX headings, fenced code, `- ` lists, paragraphs, inline `code`/links). The
  guide-vs-reference split ExDoc has, from plain Markdown files, no manifest wiring.

## v0.3.6 — 2026-08-07

- **`receive` timeouts run under WebAssembly.** `(receive … (after ms expr))` used the OS
  timer thread, which wasm has none of. The cooperative pump now fires the earliest pending
  deadline when nothing else is runnable (logical time — real delays aren't honoured, which
  is fine for a playground); messages still win over a timeout, since the pump drains the run
  queues first. Behind `cfg(target_arch = "wasm32")`; the native timer thread is unchanged.
- **`doc-catalog` recategorisation.** Reflection predicates (`bound?`, `dynamic?`, `private?`,
  `satisfies?`) → *Modules and reflection*; the ETS-style shared store (`table-*`) →
  *Processes and concurrency* (it's a concurrency primitive, not an immutable map);
  record/protocol multimethod seams (`num-add`, `ord-compare`, `to-str`, `to-seq`, `-conj`,
  …) → *Modules and reflection*; tty tests (`stdin-tty?`, `stdout-tty?`) → *System*.

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
