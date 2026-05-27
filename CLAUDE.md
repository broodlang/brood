# CLAUDE.md — working in the mylisp repo

Guidance for Claude Code (and humans) working in this project. For the broader
machine setup (Ubuntu, apt, Rust via rustup, etc.) see the global
`~/.claude/CLAUDE.md`.

## What this project is

mylisp is a small dynamic **Lisp implemented in Rust**. Its purpose is to be the
language a modern, Emacs-like, self-editing, remotely-hostable text editor is
written in. Today the repo is the **language core**; the editor, display
protocol, and server come later. Read `docs/` before making non-trivial changes
— especially `docs/architecture.md`, `docs/roadmap.md`, and `docs/decisions.md`.

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
crates/cli/src/main.rs   the `mylisp` binary (REPL + file runner)
std/prelude.lisp         standard library written in mylisp
docs/                    architecture, language, roadmap, decisions, devlog
```

## Commands

```bash
cargo build                       # build the workspace
cargo test                        # run all tests (crates/lisp/tests/basic.rs)
cargo run -p cli                  # start the REPL
cargo run -p cli file.lisp        # run a program file
cargo test -p mylisp --test basic # just the language tests
```

`make -j$(nproc)` isn't used here — it's a Cargo workspace.

## Conventions & invariants (don't break these)

- **Proper tail calls are load-bearing.** `eval` is a `'tail: loop`. When adding
  a special form that has a body or branches, evaluate all-but-last for effect
  and hand the *last* form back to the loop (`expr = …; continue 'tail;`) — see
  the `tail_of`/`tail_of_vec` helpers. Don't turn tail positions into plain
  recursion; the test `tail_calls_do_not_overflow` (sum to 1,000,000) guards
  this.
- **All heap construction goes through `value.rs` helpers** (`cons`, `list`,
  `sym`, `str_val`, …). This keeps the planned `Rc` → `gc-arena` migration
  contained (ADR-002). Don't scatter `Rc::new(...)` of `Value`s elsewhere.
- **Prefer mylisp over Rust.** If something can live in `std/prelude.lisp`
  instead of `builtins.rs`, put it there — that's what stays editable at runtime
  (ADR-006). Add a Rust builtin only when it needs Rust (I/O, primitives,
  performance).
- **Symbols are interned `u32`s.** Compare with `==`; get the spelling via
  `value::symbol_name`.
- **Truthiness:** only `nil` and `false` are falsy (`eval::truthy`).
- **Keep v0.1 dependency-free** unless a milestone genuinely needs a crate
  (ADR-005). When you do add one, note it in `docs/decisions.md`.

## When you add a feature

1. Implement it (special form in `eval.rs`, or builtin in `builtins.rs`, or
   prelude fn in `std/prelude.lisp`).
2. Add end-to-end tests in `crates/lisp/tests/basic.rs`.
3. Update `docs/language.md` (it documents the language *as implemented*).
4. Tick it off in `docs/roadmap.md`; add a dated entry to `docs/devlog.md`.
5. If it reflects a real design choice, record an ADR in `docs/decisions.md`.

## Known next steps (see roadmap)

Macros + quasiquote, dynamic variables (`defdyn`/`binding`), in-language
`try`/`catch`, map literals, and the tracing-GC migration complete the language
core (M1). After that: the editor data model (M2), display protocol + native
frontend (M3), server mode (M4), web frontend (M5).
