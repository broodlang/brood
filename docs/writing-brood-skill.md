---
name: writing-brood
description: Use when writing or editing Brood Lisp (`.blsp`) source — generating new code, modifying std/prelude, or scaffolding a project. Brood is a small immutable Lisp implemented in Rust; it differs from Clojure/Scheme/CL in ways an LLM will get wrong by default (no mutation, no loops, lists-for-code/vectors-for-data, binding patterns). Load this before producing Brood code.
---

# Writing Brood

Brood (`.blsp`) is a small, dynamic, **immutable** Lisp. The full reference is
`docs/brood-for-claude.md` (read it for depth); `std/prelude.blsp` is the
canonical example of idiomatic code. This skill is the short list of things you
will get wrong if you write Brood like Clojure, Scheme, or Common Lisp.

## The traps (these are where LLM-written Brood goes wrong)

1. **No mutation. None.** No `set!`, `setq`, atoms, cells, `vector-set!`,
   `set-car!`. Every operation returns a fresh value. The *only* mutation is
   `def` re-binding a **global** (used for hot reload). State that genuinely
   changes lives in a **process** (`spawn`/`send`/`receive`) or behind a
   Rust-backed handle — never a mutable value.

2. **No loops.** No `while`, no `for`, no `loop`/`recur`. Iterate with
   **tail-recursion + an accumulator** (TCO is guaranteed, O(1) stack), or with
   `fold` / `reduce` / `map` / `filter` / `transduce`. Deep *non*-tail
   recursion overflows the green-process stack.

3. **Lists for code, vectors for data.** Binding forms are **lists**, not
   Clojure vectors: `(let (a 1 b 2) …)`, `(for (x xs :when p) …)`,
   `(when-let (v e) …)`. `let` bindings are **flat** — `(let (a 1 b 2) …)`, not
   `(let ((a 1) (b 2)) …)`. Vectors `[ ]` are *only* for tuple values
   (`[x y]`), sequence literals (`[1 2 3]`), and tuple **patterns**.

4. **Bare symbols in patterns BIND, they don't match.** In `match` / `fn` /
   `receive` / destructuring `let`, `x` binds; to match a known value pin it
   with `~x`, a literal symbol with `'sym`, a constant with `42`/`:k`/`"s"`.

5. **Truthiness:** only `nil` and `false` are falsy. `0`, `""`, `()` are
   **truthy**. `cond`'s catch-all is `:else` (or `else`) — never `t`/`true`.

6. **Don't tuple-destructure a single-clause top-level `defn` param.** Name the
   param, unpack in the body: `(defn area (p) (let ([x y] p) (* x y)))`, not
   `(defn area ([x y]) …)`. (Anonymous `fn` in `map`/`reduce` *may*
   destructure: `(map (fn ([k v]) …) m)`.)

7. **Variadic arithmetic/comparison:** `(+ a b c)`, `(< x y z)` — use these,
   not the raw 2-arg `%add`/`%lt` primitives (those are for the prelude's own
   bootstrap). Maps have **no commas**: `{:a 1 :b 2}`.

8. **Maps are seqable; `sort` and `index-of` are polymorphic.** Three places
   where the obvious builtin already covers more than you'd guess:
   - `(map f m)` / `(filter f m)` / `(fold f acc m)` / `(reduce f acc m)` /
     `(count m)` / `(into [] m)` all walk a map as its `[k v]` pairs. No need
     for `(zip (keys m) (vals m))`; map iteration order is hash-driven, so use
     `frequencies` for order-insensitive comparisons. `seq` and `entries` make
     the coercion explicit when you want it.
   - **Count with `frequencies` + `mapcat`, not a manual scan.** To tally
     something across a collection, map each item to what it contributes and let
     `frequencies` do the counting in one pass — e.g. a grid's next-generation
     neighbour counts are `(frequencies (mapcat neighbours (keys live)))`: no
     nested loop, no mutable tally. Reach for this before hand-rolling an
     accumulator over indices.
   - `(sort coll)` is `<` for numbers but **structural lexicographic order** for
     vectors/lists, and text order for strings/keywords/symbols. So
     `(sort [[1 0] [2 1]])` Just Works — no comparator needed.
   - `(index-of coll x)` accepts a list, vector, or string (substring search)
     and returns -1 if absent. `(includes? coll x)` is the predicate version
     across lists, vectors, strings, and maps (values).

## Naming & shape (match std/)

- `foo?` predicate · `*foo*` dynamic/module var · `foo--bar` **private** helper
  · `foo->bar` conversion. Kebab-case. No `!` convention (nothing mutates).
- Tail-recursive helpers: public shell delegates to a private `name--acc`/
  `--loop` worker that carries the accumulator.
- Docstring (one-sentence summary, first line) on every public `defn`/`defmacro`;
  backticks/**bold**/`-` bullets render. Each module opens with `(defmodule name "…")`.
- Errors: `(error "fn-name: what went wrong: " value)` — lowercase, value appended.

## Use the MCP server as your coding loop

A Brood project scaffolds `.mcp.json` pointing at `nest mcp` — a Model Context
Protocol server over the **live image** (ADR-036, `docs/mcp.md`). When it's
attached, prefer it over guessing: it's how you check that the code you're about
to write actually works. Its tools:

- **`eval`** — evaluate a Brood expression in the running image. Use it to test a
  function before committing it to a file, or to reproduce a bug. *Return data
  as the result value — don't `(print …)`; that corrupts the JSON-RPC stream.*
- **`load`** — load a file into the image (re-`def`s its globals, hot-reload), so
  you can edit a `.blsp` file and immediately exercise the new definitions.
- **`lookup`** — a global's arglist, docstring, and source location. Check a
  builtin's real signature here instead of assuming.
- **`macroexpand`** — see what a macro expands to (essential when writing or
  debugging `defmacro`).
- **`format`** — format a snippet/file the way `nest format` would.
- **`callers`** — every reference to a global across the project (rename impact).

The loop: write a definition → `load` (or `eval` it) → `eval` a call against it →
`macroexpand` if it's a macro → fix → repeat. This catches the traps above
(binding-vs-matching, non-tail recursion, truthiness) at the point of writing
rather than at `nest test`.

## Don't guess the standard library

Probing for names one at a time (`rand`? `rand-int`? `random`?) burns
round-trips. Two faster moves:

- **Read the whole reference once.** `nest doc --all` prints every builtin and
  prelude fn/macro with its signature and one-line summary; `nest doc <module>`
  does the same for an opt-in module (`display`, `buffer`, `ansi`, …). With the
  MCP server attached, `apropos` / `all-globals` / `doc-search` (and `lookup`)
  answer "does X exist / how is it called?" in one call.
- **Know the reflexes that don't carry over** — what Clojure/Scheme/CL
  muscle-memory reaches for, and what Brood actually has:

  | You reach for | Brood has |
  | --- | --- |
  | `concat` | `concat` (alias of `append`) — variadic over lists *and* vectors, returns a list |
  | `conj` onto a vector | `cons` (lists); `into` / `(apply vector …)` (vectors) |
  | `set!` / `swap!` / atoms | nothing — state is a process or a Rust handle (trap #1) |
  | `loop`/`recur`, `while`, `for`-loop | tail recursion, or `fold`/`map`/`filter`/`reduce` (trap #2) |
  | a `flush` after `print` | nothing — `print` flushes stdout every call |
  | raw ANSI (`clear`+`home`, cursor moves) | `(require 'ansi)` → **call** `(ansi-clear)`/`(ansi-home)`/`(ansi-cursor r c)`/`(ansi-hide-cursor)` — these are **zero-arg functions** that *return* an escape string, so you must call them: `(print (ansi-clear))`, **never** `(print ansi-clear)` (a bare symbol prints `#<fn …>` and emits no escape). ESC is `\e`. A render loop wants `std/display` instead. |
  | a built-in RNG (`rand`) | `rng`/`rand-int`/`rand-float`/`shuffle`/`sample` — pure & seedable, return `[value next-seed]`; thread the seed through your state |
  | a set / `#{}` | `(require 'set)` → a set is a **map of `element → true`**: membership `(contains? s x)`, elements `(keys s)`, size `(count s)`; the module adds `(set coll)` (dedups), `conj`/`disj`, `union`/`intersection`/`difference`/`subset?`. No `#{}` literal or `set?` yet — test with `map?`. |

## Before finishing

- Recursion in hot/iterative paths is **tail** recursion (last thing the
  function does is the self-call). If not, rewrite with an accumulator.
- No mutation crept in. No `[ ]` in a binding/param position. No bare symbol
  meant as a literal in a pattern.
- **A TUI / animation render loop is not covered by `nest test`** — the suite
  tests pure functions, never the loop's output. Verify it by inspecting the raw
  bytes for escapes: `nest run --for 600ms 2>&1 | cat -v | grep -oE '\^\[\[[0-9;]*[A-Za-z]'`,
  not by eyeballing a piped frame (which hides a stray `#<fn …>` printed where a
  call was meant).
- New public function has a docstring. Run `nest format` and `nest test`.
