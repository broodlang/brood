# Dev log

Chronological record of work sessions. Newest at the bottom.

---

## 2026-05-27 — Project bootstrap and v0.1 language core

**Goal.** Stand up a new dynamic Lisp in Rust whose purpose is to be the
language a modern, Emacs-like, remotely-hostable, self-editing editor is written
in. First concrete target from the user: *"a light first version where
`(+ 1 2)` works."*

**Decisions taken** (full rationale in [decisions.md](decisions.md)):
- Host language: **Rust** (ADR-001), reaffirmed against C/Zig given the
  "heavily vibe-coded" constraint — memory safety is the key guardrail.
- Memory: `Rc`/`RefCell` now, tracing GC later (ADR-002).
- Cons-cell lists + separate `[ ]` vectors (ADR-003).
- Clojure-style truthiness; flat `cond` (ADR-004).
- Zero external dependencies for v0.1 (ADR-005).
- Maximise the share of the language written in mylisp itself (ADR-006).

**Built.**
- Cargo workspace: `crates/lisp` (the language) + `crates/cli` (the `mylisp`
  binary), `std/prelude.lisp`, `docs/`.
- `value.rs`: `Value` enum, thread-local symbol interner, list/vector
  constructors, structural `PartialEq`.
- `reader.rs`: recursive-descent parser (numbers, strings, symbols, keywords,
  lists, vectors, `'` quote, `;` comments).
- `env.rs`: lexical environment chain with `get`/`set_existing`/`define`/`root`.
- `eval.rs`: tree-walking evaluator built as a `'tail: loop` for **proper tail
  calls**; special forms `quote if when unless cond do def set! fn/lambda
  let/let* and or while`.
- `builtins.rs`: arithmetic, comparison/logic, list & sequence ops, higher-order
  (`map`/`filter`/`reduce`/`apply`), predicates, strings/IO, and the
  self-hosting trio `eval`/`read-string`/`load`.
- `printer.rs`: readable + display rendering that round-trips with the reader.
- `cli/main.rs`: dependency-free REPL (balanced-delimiter multiline input) and
  file runner.
- `tests/basic.rs`: 16 end-to-end tests.

**Verified.**
- `cargo build` clean; `cargo test` → 16/16 plus the lib doc-test green.
- The headline guarantee holds: a tail-recursive sum to **1,000,000** returns
  without overflowing the stack.
- Live redefinition works: redefining a global function changes behaviour
  immediately (the seed of "edit the editor on the fly").
- REPL: `(+ 1 2)` → `3`. File runner: a recursive `fib` program prints
  correctly.

**Notable bug found & fixed.** First test run: `cond` returned a wrong branch.
Root cause was a mismatch between an initial Scheme-style clause-list `cond` and
the intended flat Clojure-style pairs. Switched the implementation to flat
`test expr` pairs with `else`/`:else` fallback (ADR-004); all green after.

**Status.** v0.1 = the ✅ slice of M1 in [roadmap.md](roadmap.md). Next up to
finish M1: macros + quasiquote, dynamic variables, in-language error handling,
maps, and the GC migration.
