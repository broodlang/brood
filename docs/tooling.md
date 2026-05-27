# Editor integration (tooling contract)

Brood is meant to be the language of a self-editing editor, so being
*editor-readable* is a language concern, not an afterthought bolted onto an
editor. This document is the **contract** between the Brood CLI and any editor
front-end (today: the Emacs mode in `brood.el` / `inf-brood.el`): the exact
output formats and introspection entry points an editor can rely on.

> Status: this lands in stages.
> - **Stage 1 (done):** parseable error output (below).
> - **Stage 2 (done):** machine-readable test reporter with per-test source
>   locations; `form-pos` / `current-file` introspection (below).
> - **Stage 3 (planned):** richer introspection (`arglist`, completions) for
>   eldoc / completion-at-point / xref.

## Error output: GNU `FILE:LINE:COL:`

When the CLI runs a file (`brood file.blsp`) and evaluation fails, the error is
written to **stderr** in the GNU convention that `compilation-mode`, `flymake`,
and most editors parse out of the box:

```
FILE:LINE:COL: KIND error: MESSAGE
```

Examples:

```
examples/tour.blsp:12:5: parse error: unexpected ')'
examples/tour.blsp:3:1: unbound error: unbound symbol: nope
```

`KIND` is one of `parse`, `unbound`, `arity`, `type`, `runtime`, or absent for a
user `(throw …)` (which prints `error: …`). The process exits non-zero.

### Position precision

- **Parse errors** carry the reader's **exact** `line:col` — the character where
  parsing failed.
- **Runtime errors** carry the **start `line:col` of the enclosing top-level
  form**. This is deliberately coarse: macro expansion rewrites inner forms, so
  a precise inner position is unreliable in general, whereas the top-level form
  is always known and always clickable. (Per-form precision for specific,
  un-expanded sites — e.g. test assertions — arrives in Stage 2 via `form-pos`.)

If no position is known the CLI falls back to `FILE: message` (file still
clickable, no line).

### How this is produced

`Interp::eval_source` reads each top-level form *with* its start position
(`reader::read_all_positioned`) and tags any otherwise-unpositioned error with
that form's position (`LispError::or_pos`). `LispError` carries an optional
`error::Pos { line, col }` (1-based). The REPL path (`eval_str`) intentionally
leaves positions unset — there is no file to point into.

## Editor side (Emacs)

`M-x brood-run` (`C-c C-c`) runs the current file and `M-x brood-test`
(`C-c C-t`) runs `brood test`, both in a `brood-compilation-mode` buffer.
That mode uses the built-in `gnu` matcher (plus a position-less fallback), so
`next-error` / clicking jumps straight to the offending file and line. See
`lisp/progmodes/inf-brood.el`.
