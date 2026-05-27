# CLAUDE.md — working in the Brood repo

Guidance for Claude Code (and humans) working in this project. For the broader
machine setup (Ubuntu, apt, Rust via rustup, etc.) see the global
`~/.claude/CLAUDE.md`.

## What this project is

Brood is a small dynamic **Lisp implemented in Rust**. Its purpose is to be the
language a modern, Emacs-like, self-editing, remotely-hostable text editor is
written in. Today the repo is the **language core**; the editor, display
protocol, and server come later. Read `docs/` before making non-trivial changes
— especially `docs/architecture.md`, `docs/roadmap.md`, and `docs/decisions.md`.

## Core principle: write the language in the language

**As much of the system as possible must be written in Brood itself, not in
Rust.** This is the most important rule in this repo — it is the entire reason
the project exists (a self-editing editor is only possible if its behaviour
lives in code you can redefine at runtime).

Concretely:
- Rust provides **mechanism**; Brood provides **policy**. Use Rust only for
  what genuinely needs it: primitives the language can't bootstrap (low-level
  I/O, the rope/text engine, performance-critical kernels) and the core
  evaluator itself.
- Everything else belongs in `std/` (Brood source), not in `builtins.rs`. When
  you reach for a new Rust builtin, first ask: *can this be written in Brood on
  top of existing primitives?* If yes, do that instead.
- This applies to upcoming pieces too. The **CLI/REPL, the editor commands,
  keymaps, and UI should ultimately be Brood**, with Rust only hosting the
  thinnest necessary substrate. (The REPL is Rust today as a bootstrap; moving
  it into Brood is a goal — see `docs/roadmap.md`.)
- A Rust builtin is an admission that the language can't yet express something.
  Treat each one as a candidate to later replace with Brood once the language
  is capable enough.

## Layout

```
crates/lisp/src/
  value.rs     Value enum, symbol interner, list/vector constructors  (all heap allocation funnels through here)
  reader.rs    text -> Value
  env.rs       lexical environment chain
  eval.rs      evaluator — a `'tail: loop` for proper tail calls + special forms
  builtins.rs  functions implemented in Rust
  printer.rs   Value -> text
  error.rs     LispError / LispResult
  lib.rs       the `Interp` entry point; bundles std/prelude.lisp
crates/cli/src/main.rs   the `brood` binary (REPL + file runner)
std/prelude.lisp         standard library written in Brood
docs/                    architecture, language, roadmap, decisions, devlog
```

## Commands

```bash
cargo build                       # build the workspace
cargo test                        # Rust tests + the Brood suite (tests/suite.lisp)
cargo run -p cli                  # start the REPL  (or: ./bin/cli)
cargo run -p cli file.lisp        # run a program file
./bin/cli tests/suite.lisp        # the in-language test suite (does (require 'test))
```

`make -j$(nproc)` isn't used here — it's a Cargo workspace.

## Conventions & invariants (don't break these)

- **Proper tail calls are load-bearing.** `eval` is a `'tail: loop`. When adding
  a special form that has a body or branches, evaluate all-but-last for effect
  and hand the *last* form back to the loop (`expr = …; continue 'tail;`) — see
  the `tail_of`/`tail_of_vec` helpers. Don't turn tail positions into plain
  recursion; the test `tail_calls_do_not_overflow` (sum to 100,000) guards
  this.
- **All heap construction goes through `value.rs` helpers** (`cons`, `list`,
  `sym`, `str_val`, …). This keeps the planned `Rc` → `gc-arena` migration
  contained (ADR-002). Don't scatter `Rc::new(...)` of `Value`s elsewhere.
- **Prefer Brood over Rust** — see the "write the language in the language"
  principle above (ADR-006). If something can live in `std/` instead of
  `builtins.rs`, put it there. Add a Rust builtin only when it genuinely needs
  Rust.
- **Favor the simplest user-facing design; defer power features** (ADR-011).
  When a feature has a simple form and a powerful-but-complex form, ship the
  simple one and defer the rest until a concrete need justifies it. Additive
  features cost nothing to delay; every knob is a tax on every user, forever.
- **Keep the language as small as possible.** Minimize the *core* — special
  forms and evaluator semantics — above all. When a feature can be a macro over
  a primitive function instead of a new special form, do that (e.g. `try`/`catch`
  is a macro over a `%try` primitive, not a special form). Primitives are just
  Rust functions; special forms are language. Prefer adding the former.
- **Symbols are interned `u32`s.** Compare with `==`; get the spelling via
  `value::symbol_name`.
- **Truthiness:** only `nil` and `false` are falsy (`eval::truthy`).
- **No Rust dependencies in the runtime.** The `brood` lib crate stays
  dependency-free — hand-roll any data structures the runtime needs (our own
  substrate), don't pull in crates like `boxcar`/`dashmap`/`gc-arena`. Dev/UX
  deps in the **CLI** crate (e.g. `rustyline`) are fine. (Supersedes ADR-005.)
- **Instances are independent; code updates do not propagate.** Each
  process/runtime has its own mutable global function table (seeded from a
  shared, read-only prelude). Redefining a function updates *that* instance only
  — never other running processes or connected runtimes. Single-instance
  hot-reload works via late binding (re-eval the `def`); there is deliberately
  **no** shared mutable global / cross-process code propagation.

## When you add a feature

1. Implement it (special form in `eval.rs`, or builtin in `builtins.rs`, or
   prelude fn in `std/prelude.lisp`).
2. Add tests — an `(assert= …)`/`(is …)` inside a `deftest` in `tests/suite.lisp`
   (in-language, via the `std/test.lisp` framework), and/or a Rust case in
   `crates/lisp/tests/basic.rs`.
3. Update `docs/language.md` (it documents the language *as implemented*).
4. Tick it off in `docs/roadmap.md`; add a dated entry to `docs/devlog.md`.
5. If it reflects a real design choice, record an ADR in `docs/decisions.md`.

## Known next steps (see roadmap)

Macros + quasiquote, dynamic variables (`defdyn`/`binding`), in-language
`try`/`catch`, map literals, and the tracing-GC migration complete the language
core (M1). After that: the editor data model (M2), display protocol + native
frontend (M3), server mode (M4), web frontend (M5).
