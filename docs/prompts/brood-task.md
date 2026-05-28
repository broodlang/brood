You're working in a Brood project (a `.blsp` Lisp). Before generating
Brood code, fetch these three resources in order:

1. `brood://docs/brood-for-claude` — the pocket reference for the idioms
   that differ from other Lisps.
2. `brood://docs/incarnations` — short index of what tripped up prior
   agents. Read the most recent entry's "What I'd tell next-me" before
   starting; full findings linked from each entry.
3. `CLAUDE.md` at the project root — project-specific conventions
   (commands, gotchas, code style).

**Brood essentials**
- Data is immutable; `def` is the only mutation (rebinds globals,
  Erlang-style hot reload).
- Loops are tail recursion or processes (`spawn`/`receive`/`send`). No
  `while`, no `set!`.
- Truthiness: only `nil` and `false` are falsy.
- Modules: `(provide 'foo)` + `(require 'foo)`; symbols are interned,
  compare with `=`.
- **Errors are structured.** A `(try … (catch e …))` on a kernel error
  rebinds `e` to a map: `{:kind :unbound :code "E0010" :message "…"
  :file … :line … :col …}`. Branch on `:kind` (`:parse` / `:unbound` /
  `:arity` / `:type` / `:runtime`) or the stable `:code` (the full list
  is at `brood://docs/error-codes`). User throws (`(throw v)`) keep
  `(catch e e) → v` — only kernel errors get the wrapper.

**MCP tools (use these to interact with the live image)**
- `eval` — try expressions. State (a `def`, a `spawn`) persists between
  calls. Avoid `(println …)` inside an eval for now — `print` writes to
  the dispatcher's stdout and corrupts the JSON-RPC stream; return data
  via the `:value` field instead. (A `:stdout` field is on the roadmap
  once the `*out*` dynvar work lands.)
- `lookup` — `:arglist` + `:doc` + `:source-location` for a name. No
  quote: `{:name "map"}`.
- `macroexpand` — see what a macro lowers to. Useful for `when-let`,
  `cond`, `match`, and anything from `hatch` (`defprocess`, `cast`, `!`,
  `gen-call`).
- `format` — reformat source idempotently.
- `load` — load a `.blsp` file into the live image.
- `check` — advisory type-check; structured diagnostics back.
- `run-tests` — structured runner result with per-test pass/fail.
- `processes` — live pids.

**When you finish a non-trivial session**, append a one-paragraph entry
to `docs/incarnations.md` (the format is at the top of that file) and
drop your full findings into `docs/<your-slug>.md`. The next agent
reads what you wrote first — make it count.
