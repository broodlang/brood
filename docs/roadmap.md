# Roadmap

The destination is a modern, Emacs-like editor written in Brood, runnable
locally as a fast native app and remotely as a server for other editor
instances. We get there in milestones. Each milestone is shippable and useful on
its own.

Legend: ✅ done · 🟡 in progress · ⬜ not started

---

## M1 — The language core

A solid, self-editable Lisp. This is the foundation everything else stands on.
The detailed Stage-1 completeness checklist ("what's left to be a full,
standalone Lisp") lives in the top-level [`ROADMAP.md`](../ROADMAP.md). A major
**parallel core track** — Erlang-style green-process concurrency across all
cores — is designed in [`concurrency.md`](concurrency.md) and tracked in
`ROADMAP.md`.

- ✅ Reader (text → values): numbers, strings, symbols, keywords, lists, vectors, `'` quote, comments
- ✅ Value model with interned symbols; cons-cell lists
- ✅ Lexical environments + closures
- ✅ Tree-walking evaluator with **proper tail calls**
- ✅ Special forms: `quote if when unless cond do def set! fn/lambda let/let* and or while`
- ✅ Builtins: arithmetic, comparison, lists/sequences, higher-order, predicates, strings/IO
- ✅ Self-hosting primitives: `eval`, `read-string`, `load`
- ✅ Prelude written in Brood
- ✅ REPL + file runner
- ✅ End-to-end test suite (incl. 100,000-deep tail recursion, live redefinition)
- ✅ **Primitive-kernel refactor**: `+ - * / < > = map reduce …` are defined in
  Brood (`std/prelude.lisp`) over a small Rust kernel (ADR-008)
- ✅ **Macros** (`defmacro`, `macroexpand`/`macroexpand-1`, `gensym`); `defn` and
  the `->`/`->>` threading macros are now defined *in Brood* (`std/prelude.lisp`)
- ✅ **Quasiquote** — Clojure-style `` ` `` / `~` / `~@` (ADR-009)
- ✅ **Parameter grammar** — `required` + `&optional` (with defaults) + `& rest`,
  in the closure calling convention (`fn`/`lambda`/`defn` all share it).
  `&key` (named args) is designed but **deferred for simplicity** (ADR-011) —
  additive when the editor command API needs it.
- ⬜ **Dynamic variables** (`defdyn` / `binding`) for editor config
- ✅ **Error handling** — `throw` + `%try` primitives; `try`/`catch` + `error`
  in the prelude (no new special forms — ADR-011)
- ⬜ **Maps** (`{ }` literals, `get`/`assoc`)
- 🟡 **Memory reclamation.** `Send` arena handles replaced `Rc` (done). Step 1 of
  reclamation is **arena reset at top-level boundaries** (ADR-016): `eval_str` and
  the REPL truncate the LOCAL heap back after each form — bounds a long
  session/REPL (demo: ~712 MB growing → ~78 MB flat). Still ⬜: a general tracing
  GC for *mid-evaluation* / never-returning loops, which needs the evaluator's
  roots to be scannable (the explicit-value-stack VM that step 4b also needs —
  they're coupled). `gc-arena` no longer the presumed path. See `memory-model.md`.
- 🟡 Nicer REPL — `rustyline` line editing (arrow keys, history, Emacs bindings)
  is in; richer completion/highlighting still to come
- ⬜ **Self-host the CLI/REPL in Brood** — once the language can express it, the
  read-eval-print loop should be Brood source on a thin Rust substrate, not
  Rust. (See the core principle in `CLAUDE.md`.)

> v0.1 is the ✅ slice above: enough to be a real, usable language. The ⬜ items
> complete M1.
>
> **Overarching principle:** as much of the system as possible is written in
> Brood itself — Rust is mechanism, Brood is policy. Every Rust builtin is a
> candidate to later replace with Brood. This holds for the CLI, the editor
> commands, keymaps, and UI as the language grows capable enough.

## M2 — Editor data model

The text-editing substance, exposed to Brood.

- ⬜ Rope-backed buffers (`ropey`) — efficient edits on large files
- ⬜ Points, marks, regions; multiple buffers
- ⬜ Editing primitives as builtins: `insert`, `delete`, `goto`, `search`, …
- ⬜ Buffers as first-class Brood values
- ⬜ Do the GC migration here if not already done

## M3 — Display protocol + native local frontend

The seam that makes remoteability free later (see architecture.md).

- ⬜ A serialisable display protocol (render ops: lines, faces/styles, cursor, minibuffer)
- ⬜ Input events (keys) flowing back in
- ⬜ A native, in-process frontend (terminal via `crossterm`, or a GPU window) — the fast local path
- ⬜ Keymaps and interactive commands defined in Brood

## M4 — Server / daemon mode

- ⬜ The same runtime listens on a socket and serves the M3 protocol
- ⬜ Remote editor instances attach (the Emacs `--daemon` / `emacsclient` model)
- ⬜ One core, multiple attached frontends

## M5 — Web frontend

- ⬜ Implement the display protocol over WebSocket
- ⬜ Browser renderer (DOM or canvas)

---

## Guiding principles

- **Keep policy in Brood, mechanism in Rust.** If something *can* live in the
  language instead of the runtime, it should — that's what stays editable at
  runtime.
- **The frontend is a protocol.** Local-native and remote-web are the same code
  path with different transports.
- **Every milestone is usable.** No "big bang" rewrites.
