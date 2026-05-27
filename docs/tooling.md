# Editor integration (tooling contract)

Brood is meant to be the language of a self-editing editor, so being
*editor-readable* is a language concern, not an afterthought bolted onto an
editor. This document is the **contract** between the Brood CLI and any editor
front-end (today: the Emacs mode in `brood.el`): the exact output formats and
introspection entry points an editor can rely on.

There is **one output format**, always on — structured, GNU-anchored, and equally
readable by humans, LLMs reading a captured run, and editors. No "human vs
machine" mode flag.

> Status: this lands in stages.
> - **Stage 1 (done):** parseable error output (below).
> - **Stage 2 (done):** structured test reporter with per-assertion source
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
  is always known and always clickable.

When the file and position are both known the CLI also prints the offending
**source line and a caret** under the column:

```
examples/tour.blsp:12:5: parse error: unclosed list (opened here)
    (defn f (x
            ^
```

If no position is known the CLI falls back to `FILE: message` (file still
clickable, no line). `LispError` carries optional `error::Pos { line, col }` and
`file`; `Interp::eval_source` and `load` tag errors with the enclosing top-level
form's position. The REPL path (`eval_str`) leaves them unset — nothing to point
into.

## Test output: a structured block per failure

`brood test` reports each failed assertion as a GNU-anchored block — the first
line is the editor-parseable `FILE:LINE:COL:`, the rest are indented labelled
fields:

```
tests/math_test.blsp:2:25: test failed: math › adds
    assert: (assert= (+ 1 1) 3)
    actual: 2
    expect: 3
```

The anchor's `LINE:COL` is **per-assertion** (not just the test's line): each
assertion macro captures its own location at macro-expansion time. A passing run
prints only the summary line. There is no progress/colour output, and no mode
flag — this *is* the format.

### How this is produced

The reader records every list form's `line:col` in a heap side-table; `(form-pos
form)` returns it (or `nil`), and `(current-file)` returns the file `load` is
reading. The `is` / `assert=` / `refute` / `assert-error` macros (in
`std/test.blsp`) call these at expansion — while the original form still exists,
before it macro-expands — and embed the `(file line col)` into a structured
failure record `(loc detail-lines)`. The runner prints those records.

## Editor side (Emacs)

`M-x brood-run` (`C-c C-c`) runs the current file and `M-x brood-test`
(`C-c C-t`) runs `brood test`, both in a `brood-compilation-mode` buffer (run
through a pipe, so output is clean and un-coloured). That mode uses the built-in
`gnu` matcher plus a position-less fallback, so `next-error` / clicking jumps
straight to the failing line. Everything lives in `lisp/progmodes/brood.el`.
