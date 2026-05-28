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
> - **Stage 3 (in progress):** richer introspection (`arglist`, completions) for
>   eldoc / completion-at-point / xref — generalised across editors by a
>   language server (`brood-lsp`), wired into Emacs via Eglot. Design:
>   [`lsp.md`](lsp.md) / ADR-025.

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

`nest test` reports each failed assertion as a GNU-anchored block — the first
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

## Running a project: `nest run`

`nest run [args…]` runs the project's entry point. The entry is configured by
the optional `:main` key in `project.blsp` and defaults to module `main`, fn
`main` — so a project scaffolded by `nest new` runs out of the box without
declaring anything:

```
(project
  :name    "myapp"
  :version "0.1.0")          ; :main omitted -> (main main)

;; override the fn name:
(project ... :main '(main start))

;; or just the module (fn name defaults to `main`):
(project ... :main 'cli)
```

Extra positional args after `run` are passed to the entry fn as strings, so
`nest run alpha beta` calls `(main "alpha" "beta")`. The entry can be no-arg
(`(defn main () …)`) or variadic (`(defn main (& args) …)`); a fixed-arity
mismatch surfaces as a normal Brood error.

`run-project` (in `std/project.blsp`) walks from `cwd` up to `project.blsp`,
loads the manifest (which may override `*project-main*`), puts the project's
source paths on `*load-path*`, `require`s the entry module — pulling in
everything it transitively requires — then `apply`s the entry fn to the CLI
args. A missing project, an unbound entry fn, or a non-callable entry are
reported as editor-parseable errors and exit non-zero.

## Formatting source: `nest format`

`nest format` rewrites every `.blsp` under the project's source + test paths in
place using a single, opinionated style. `nest format --check` does the same
walk read-only and exits non-zero if anything would change — the CI mode. Both
are policy in Brood (`std/format.blsp`); the only new Rust is the `parse-source`
primitive, which returns the lossless CST as nested vectors so the walker can
see whitespace and comments.

The layout, in one paragraph: every form is emitted on a single line if it fits
within the width budget (`*format-width*`, 100 cols); otherwise it breaks across
lines with each body argument on its own line at +2 indent. A small table of
*header counts* (`*format-headers*`) keeps a fixed prefix of args on the first
line of certain forms — `defn` keeps `name params`, `let` keeps the bindings
list, `if`/`when`/`unless` keep the predicate, etc. — so the body indents under
a recognisable header. Comments inside a list force the multi-line shape; they
re-emit on their own line at the surrounding indent, with their original text
preserved verbatim. Blank lines between top-level forms (or top-level comments)
are preserved when the author left one; runs of 3+ blanks collapse to a single
blank.

### Idempotency is a contract

`(= (format-source (format-source s)) (format-source s))` for every input.
That's the property `tests/format_test.blsp` asserts on a grab-bag of shapes
*and* on the full prelude — the largest single Brood source in the tree. A
break of idempotency is a bug, not a stylistic preference.

### Comment preservation, in detail

The CST records each `;` comment as a `[:comment "...;...\n"]` node. The
formatter strips the trailing newline (so adjacent blocks join cleanly), then
re-emits the comment on its own line at the current indent. A comment between
the head and the first body item of a list lives inside the list — it goes on a
line of its own at the body indent. A comment between top-level forms behaves
like a top-level block: it gets its own line, and blank lines around it survive.

### What is *not* in scope (yet)

- **Width is not configurable** at the CLI. Set `*format-width*` from a
  `project.blsp` hook if you need a different budget.
- **No "align after head"** for generic calls — every overflow arg goes to
  `+2`, never to `(head)`-column. Simpler, idempotent under rename of the head.
- **No `if`-cascade recognition.** A hand-aligned `(if a 1 (if b 2 (if c 3)))`
  re-emits as a nested staircase. If you wrote it as `cond` it would stay flat;
  the formatter is not in the business of rewriting forms.

## Documentation output: Markdown from `nest doc`

`nest doc [module]` emits Markdown documentation to stdout: with no operand it
documents the whole project (every source file under it); with a module name it
documents that one module (a baked-in std module, or one on `*load-path*`). Each
module renders as a `# module <name>` heading, the module docstring (the file's
leading string form, if any), a `## Definitions` section with each function /
macro as a `### (name args…)` heading plus its docstring, and a `## Variables`
list of non-function bindings.

### How this is produced

Policy is Brood (`std/docs.blsp`); Rust supplies only the mechanism. The tool
**loads the module and introspects it** rather than parsing source: it snapshots
`(global-names)`, loads the module, and the new names are what it defined — read
back via the existing `(doc f)` / `(arglist f)`. The module docstring is read
from source with `slurp` + `read-string` (a leading string form is discarded on
load, so it can't be recovered by introspection). This reuses the canonical
docstring machinery and is one-shot, unlike the continuously-running LSP, which
must never evaluate user code (see `docs/lsp.md`).

One consequence of the load-and-diff attribution: a module **already loaded**
before the snapshot yields an empty delta, so it can't be re-documented in the
same process (this is why `docs` lazily requires `project`). Accurate
attribution independent of load order is the job of the static CST walk planned
in `docs/lsp.md`.

## Editor side (Emacs)

Everything lives in one file, `lisp/progmodes/brood.el` (`brood-mode`, derived
from `lisp-data-mode`). It splits its external commands along the same
brood/nest line as the CLI (ADR-028): **language** commands shell out to
`brood-program-name`, **project** commands to `nest-program-name`.

Run-in-a-compilation-buffer commands (clean, un-coloured output through a pipe;
`brood-compilation-mode` uses the built-in `gnu` matcher plus a position-less
fallback, so `next-error` / clicking jumps straight to the failing line):

| Command | Key | Runs |
|---|---|---|
| `brood-run`  | `C-c C-c` | `brood FILE` — the current file |
| `brood-test` | `C-c C-t` | `nest test` — discover + run the suite |
| `brood-doc`  | `C-c C-d` | `nest doc [module]` (prefix arg prompts) |
| `brood-new`  | `C-c C-n` | `nest new NAME` — scaffold a project |

`brood-toggle-test` (`C-c C-,`) jumps between a source file and its test —
`src/REL.blsp` ⟷ `tests/REL_test.blsp`, resolved against the project root
(nearest `project.blsp`) — offering to create the counterpart if it's missing.
There's also an inferior REPL (`run-brood`) with the usual `brood-send-*`
eval-in-REPL commands.

### Language server (Eglot)

Stage 3 — richer introspection (hover, completion, signature help, go-to-def,
references, live diagnostics) — is delivered by the `brood-lsp` server (see
[`lsp.md`](lsp.md)), not by bolting features onto the mode. `brood.el` registers
it with Eglot (`eglot-server-programs`, via a `brood--eglot-contact` function so
a custom `brood-eglot-server-program` is honoured at connect time). `M-x
brood-eglot` (or plain `M-x eglot`) connects; add `eglot-ensure` to
`brood-mode-hook` for auto-connect. Once connected, Eglot supplies the xref,
eldoc, capf and flymake backends — the cross-editor generalisation of the GNU
error contract above.
