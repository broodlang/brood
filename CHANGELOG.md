# Changelog

All notable changes to the Brood toolchain (`brood`, `nest`, `brood-lsp`) are
recorded here. Versions follow [semver](https://semver.org); the full
engineering narrative lives in [`docs/devlog.md`](docs/devlog.md).

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
