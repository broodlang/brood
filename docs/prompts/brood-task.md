You're working in a Brood project (a `.blsp` Lisp). Before generating
Brood code, read:

1. `brood://docs/brood-for-claude` — the pocket reference for the idioms
   that differ from other Lisps (the project also has it on disk as
   `docs/brood-for-claude.md`).
2. `CLAUDE.md` at the project root — project-specific conventions
   (commands, gotchas, code style).

**Brood essentials**
- Data is immutable; `def` is the only mutation (rebinds globals,
  live code reload across running processes). State that changes lives in
  a process or a `table`.
- No loops: tail recursion, `fold`/`reduce`/`map`, or a `letrec` local
  loop. No `while`, no `set!`, no `loop`/`recur`.
- The collection comes first, the callback last: `(map xs f)`,
  `(reduce xs init f)`.
- Lists for code, vectors for data: `(let (a 1 b 2) …)`, `(fn (x y) …)`.
- Truthiness: only `nil` and `false` are falsy — and `()` is `nil`.
- A known failure (`string/->number "x"`) is a returned, truthy
  `failure` value; test it with `failure?`.
- Modules: `(defmodule foo …)` in the file, `(:use foo)` or qualified
  `foo/bar` to reach it. Output is `io/puts`; there is no `println`.
- **Errors are structured.** A `(try … (catch e …))` on a kernel error
  binds `e` to a map: `{:kind :unbound :code "E0010" :message "…"
  :file … :line … :col …}`. Branch on `:kind` or the stable `:code`
  (the `error` tool explains one; the full list is
  `brood://docs/error-codes`); `(catch {:kind :type} …)` catches one
  kind. A user `(throw v)` keeps `(catch e e) → v`.

**MCP tools (use these against the live image instead of guessing)**
- `eval` — try expressions. State (a `def`, a `spawn`) persists between
  calls. Printed output comes back as a second `[captured stdout]` block.
- `write` / `edit` — create or change a project file; a `.blsp` file is
  then loaded and checked, so the result carries its diagnostics. Editing
  a file another way leaves the image stale until you `load` it.
- `load` — load a `.blsp` file into the live image.
- `check` (the project) / `check-source` (a snippet) — advisory
  type-check, structured diagnostics back.
- `run-tests` — the project suite, per-test pass/fail.
- `lookup` — arglist, docstring, type and source location for a name
  (no quote: `{"name": "reduce"}`). Check argument order here.
- `apropos` / `doc-search` / `all-globals` — does it exist, and what is
  it called?
- `find-pattern` — the Brood idiom for a task ("loop", "mutable state",
  "build a string") before you reach for another Lisp's habit.
- `error` — what an error code means and the idiomatic fix.
- `abilities` / `ability` — the generic-function interfaces and who
  implements them, before you write an `impl`.
- `macroexpand`, `format`, `callers` (every use of a global), `bench`,
  and the runtime views `processes` / `process-info` / `node` / `vitals` /
  `watch-runtime`.
