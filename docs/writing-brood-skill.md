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

8. **Maps are seqable; `sort`/`index-of` are polymorphic.** `map`/`filter`/`fold`/
   `reduce`/`count`/`into` all walk a map as its `[k v]` pairs (no
   `(zip (keys m) (vals m))`); order is hash-driven, so compare with
   `frequencies`. `(sort coll)` uses structural lexicographic order for
   vectors/lists — `(sort [[1 0] [2 1]])` needs no comparator. `index-of` /
   `includes?` work on lists, vectors, and strings (substring).

## Naming & shape (match std/)

- `foo?` predicate · `*foo*` dynamic/module var · `foo--bar` **private** helper
  · `foo->bar` conversion. Kebab-case. No `!` convention (nothing mutates).
- Tail-recursive helpers: public shell delegates to a private `name--acc`/
  `--loop` worker that carries the accumulator.
- Docstring (one-sentence summary, first line) on every public `defn`/`defmacro`;
  backticks/**bold**/`-` bullets render. Each module opens with `(defmodule name "…")`.
- Errors: `(error "fn-name: what went wrong: " value)` — lowercase, value appended.

**Modules are namespaces (ADR-065).** `(defmodule name …)` compiles the file into
namespace `name`: `def`/`defn` define `name/foo`. To call another module's names
**bare**, add a `(:use mod)` clause to the header (`(defmodule app "…" (:use
editor/display) (:use test))`); a plain `(require 'mod)` only *loads* it, leaving names
qualified (`mod/foo`), and `(:require …)` is **not** a clause (silently ignored —
bare calls then fail `unbound symbol`). Earmuffed `*foo*` names are ambient/bare,
never namespaced. From outside a module (REPL, `nest mcp` eval) reach a `defn`
qualified: `(life/step …)`.

## Use the MCP server as your coding loop

A Brood project scaffolds `.mcp.json` pointing at `nest mcp` — a Model Context
Protocol server over the **live image** (ADR-036, `docs/mcp.md`). When it's
attached, prefer it over guessing: it's how you check that the code you're about
to write actually works. Its tools:

- **`eval`** — evaluate a Brood expression in the running image. Use it to test a
  function before committing it to a file, or to reproduce a bug. *Return data
  as the result value — don't `(print …)`; that corrupts the JSON-RPC stream.*
- **`load`** — load a file into the image (re-`def`s its globals, hot-reload).
  The image is a separate world from disk: after editing a file, `eval` sees the
  *old* defs until you `load` it. Reflex: edit → `load` → `eval`.
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
  | raw ANSI (`clear`/`home`/cursor) | `(:use editor/ansi)` (bare `(require 'editor/ansi)` leaves names qualified) → `(ansi-clear)`/`(ansi-home)`/`(ansi-cursor r c)` are **zero-arg fns returning an escape string** — call them: `(print (ansi-clear))`, never `(print ansi-clear)` (prints `#<fn …>`). A render loop wants `std/display`. |
  | a built-in RNG (`rand`) | `rng`/`rand-int`/`rand-float`/`shuffle`/`sample` — pure & seedable, return `[value next-seed]`; thread the seed through your state |
  | a set / `#{}` | `(:use set)` (bare `(require 'set)` leaves names qualified) → a set is a **map of `element → true`**: membership `(contains? s x)`, elements `(keys s)`; adds `(set coll)`, `conj`/`disj`, `union`/`intersection`/`difference`/`subset?`. No `#{}` literal or `set?` — test with `map?`. |

## When to reach for a process (vs staying pure)

Pure functions are the default, but reach for a **process**
(`spawn`/`send`/`receive`) when you have:

- **Long-lived evolving state** — a counter, cache, or session. A process holding
  state in its `receive` loop is *the* way to express mutable state (no
  atoms/cells — trap #1); the packaged form is a gen-server actor (`std/hatch`).
- **CPU fan-out across cores** — split the work, `spawn` a worker per band, fan
  back in with `receive`. `(bench …)` the sequential version first; small inputs
  won't beat the spawn + copy-on-send overhead.
- **I/O multiplexing** — several blocking sources at once: one process per source,
  a coordinator `receive`s.

Otherwise **stay pure** — a tail loop or `fold`/`map` is simpler and easier to test.

Messages **deep-copy** across per-process heaps (share-nothing), so a `send`-ed
value is independent in the receiver — no shared-mutation hazards. Test
concurrency with spawn-N-then-collect (fan out, `receive`, assert on the
aggregate).

## When Brood crashes (a Rust panic)

A Rust-level panic — a *kernel* fault (use-after-GC tripwire, a heap index, a
runtime invariant), not your code raising — is appended to
**`.brood_crash_dump`** in the working directory: a `=== brood crash dump ===`
block with the timestamp, thread, the `panic: …` line, and a full backtrace
(`RUST_BACKTRACE` is forced on, so the trace is always there). stderr also prints
`brood: crash report appended to .brood_crash_dump`. The file is **append-only**,
so a burst of worker-thread panics all land — read the **last** block. It catches
panics, **not** `SIGSEGV`: a coroutine stack overflow surfaces as a normal
`recursion too deep` error, not a dump.

If Brood *itself* crashes (as opposed to your program erroring), that's a runtime
bug — write up a short report rather than working around it:

1. **Minimise** to the smallest `.blsp` that still reproduces, and make it
   deterministic. The usual shapes: a loop that `load`s + allocates (hot-reload /
   GC churn), or a spawn-N fan-out (scheduler races).
2. **Capture** the last dump block from `.brood_crash_dump`, plus the runtime
   version — `git -C <brood-src> rev-parse --short HEAD` — and whether the binary
   is debug or release.
3. **Localise** with the GC/engine knobs: each one that flips the verdict
   (crash ↔ clean) names a subsystem.
   - `BROOD_GC_STRESS=1` — collect at every safepoint; makes a GC race fire
     deterministically.
   - `BROOD_GC_VERIFY=1` — walk the live graph at every safepoint (debug builds);
     on a bad root it names the root→cell path.
   - `BROOD_VM=0` — run the tree-walker instead of the compiling VM. Still
     crashes? → not VM-specific. Only with the VM? → the compiled path.
   - `BROOD_RT_GC_FLOOR=100000000` — effectively disables runtime-region
     compaction; clean with it set ⇒ the runtime collector is implicated.
   - `nest test -j 1` (or `--max-parallel 1`) — serialise the scheduler to rule
     it in or out of a concurrency crash.
4. **File it**: save the repro + the dump block + which knobs change the verdict
   + the bisected subsystem to `docs/known-issues.md` in the brood source tree
   (newest first), or alongside the project if you don't have the source.

## Before finishing

- Recursion in hot/iterative paths is **tail** recursion (last thing the
  function does is the self-call). If not, rewrite with an accumulator.
- No mutation crept in. No `[ ]` in a binding/param position. No bare symbol
  meant as a literal in a pattern.
- **A TUI / animation loop is not covered by `nest test`** (it tests pure
  functions, never loop output) — verify the emitted escapes. A full-screen
  `term-enter` TUI needs a real terminal: piped it dies (`os error 6`), so wrap
  it in a pty —
  `script -qec "nest run --for 800ms" /dev/null 2>&1 | cat -v | grep -oE '\^\[\[[0-9;]*[A-Za-z]' | sort | uniq -c`.
- New public function has a docstring. Run `nest format` (whole-tree, **no file
  arg**) and `nest test`. Keep `;` comments **out of vector/map literals and off
  `cond` clauses** — the formatter shuffles them (a trailing comment on a `cond`
  clause migrates to its own line between clauses); annotate above the form.
