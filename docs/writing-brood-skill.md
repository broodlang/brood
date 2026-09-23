---
name: writing-brood
description: Use when writing or editing Brood Lisp (`.blsp`) source — generating new code, modifying std/prelude, or scaffolding a project. Brood is a small immutable Lisp implemented in Rust; it differs from Clojure/Scheme/CL in ways an LLM will get wrong by default (no mutation, no loops, collection-first argument order, lists-for-code/vectors-for-data, binding patterns, failures as values, runtime-enforced `sig`s). Load this before producing Brood code.
---

# Writing Brood

Brood (`.blsp`) is a small, dynamic, **immutable** Lisp. The full reference is
`docs/brood-for-claude.md` (read it for depth); `std/prelude/*.blsp` is the
canonical example of idiomatic code. This skill is the short list of things you
will get wrong if you write Brood like Clojure, Scheme, or Common Lisp.

## The traps (these are where LLM-written Brood goes wrong)

1. **No mutation. None.** No `set!`, `setq`, atoms, cells, `vector-set!`,
   `set-car!`. Every operation returns a fresh value. The *only* mutation is
   `def` re-binding a **global** (used for hot reload). State that genuinely
   changes lives in a **process** (`spawn`/`send`/`receive`, or a `gen` server) or a
   `table` — never a mutable value.

2. **No loops — no `while`, no `for`-loop, no `loop`/`recur`.** Iterate with
   **tail recursion + an accumulator** (proper tail calls give O(1) stack, including
   calls to *other* functions) or the combinators `fold` / `reduce` / `map` /
   `seq/filter`. A *local*, self-contained loop is a `letrec`-bound closure called by
   name — `(letrec (go (fn (i acc) … (go …))) (go 0 0))` — which closes over the
   enclosing scope (thread only the changing state). Deep *non*-tail recursion
   overflows the green-process stack. (`for` exists, but it is a *comprehension*.)

3. **The collection comes FIRST, the function LAST.** `(map xs f)`, `(fold xs init f)`,
   `(reduce xs init f)` / `(reduce xs f)`, `(seq/filter xs pred)`, `(sort-by xs key-fn)`,
   `(seq/find xs pred)` — the reverse of Clojure's `(map f xs)`. Predicates are data-first
   too: `(string/starts-with? s "#")`, `(contains? m k)`, `(index-of coll x)`. Writing
   `(map inc xs)` fails loudly (a function where a collection belongs); a callback's own
   parameters keep the familiar order — `(fold xs 0 (fn (acc x) (+ acc x)))`.

4. **Lists for code, vectors for data.** Binding forms are **lists**, not
   Clojure vectors: `(let (a 1 b 2) …)`, `(for (x xs :when p) …)`,
   `(when-let (v e) …)`. `let` bindings are **flat** — `(let (a 1 b 2) …)`, not
   `(let ((a 1) (b 2)) …)`. A vector in a binding-container position is an error.
   Vectors `[ ]` are *only* for tuple values (`[x y]`), sequence literals
   (`[1 2 3]`), and tuple **patterns**.

5. **Bare symbols in patterns BIND, they don't match.** In `match` / `fn` /
   `receive` / destructuring `let`, `x` binds; to match a known value pin it
   with **`^x`** (*not* `~x`, which is quasiquote's unquote and is rejected in a
   pattern), a literal symbol with `'sym`, a constant with `42`/`:k`/`"s"`.

6. **Truthiness:** only `nil` and `false` are falsy. `0`, `-1`, `""`, `[]`, `{}`, `#{}`
   are **truthy** — but the **empty *list* is falsy** (`()` ≡ `nil`), so test
   emptiness with `(empty? x)`, never a bare `(if a-maybe-list …)`. `cond`'s
   catch-all is `:else` (or `else`) — never `t`/`true`. The sentinel trap:
   `index-of` answers **-1** for "absent", so `(when (index-of s "x") …)` is always
   true — test `(>= (index-of s "x") 0)`, or `(includes? s "x")`. `nest check` flags a
   condition that can never be false.

7. **A known failure is a returned VALUE, not a raise.** `(string/->number "abc")`,
   the `encoding` decoders and `datetime/parse-*` return a `failure` — and a failure is
   **truthy**, so `(or (string/->number s) 0)` yields the failure, not `0`. Test with
   `(failure? x)`, read with `(error-message x)`; stop on the first one with `ok->`
   (a pipe) or `with` (a `let`). Bugs still raise — `try`/`catch` those.
   `(catch e body…)` takes ONE bare binder, never Clojure's `(catch Type e …)`.

8. **Don't tuple-destructure a single-clause top-level `defn` param.** Name the
   param, unpack in the body: `(defn area (p) (let ([x y] p) (* x y)))`, not
   `(defn area ([x y]) …)`. (Anonymous `fn` in `map`/`fold` *may*
   destructure: `(map m (fn ([k v]) …))`.)

9. **Many familiar names live in a module, and bare they are unbound.** Output is
   `io/puts`/`io/write`/`io/inspect` (no `print`/`println`). `min`/`max`/`quot`/`rem`/
   `mod`/`floor`/`abs`/`sqrt`/`even?`/`odd?` are `math/…`; `zip`/`subvec`/`remove-nth`/
   `repeatedly`/`frequencies`/`group-by`/`find`/`distinct`/`filter`/`reject` are `seq/…`;
   the whole string surface is `string/…` (`string/length`, `string/join`,
   `string/format`, `string/interp` — interpolation, `(string/interp "x={x}")`); the
   clock is `os/now`. A qualified reference loads its module on first use; there is no
   `require` form. Still bare: `map fold reduce count first rest conj into get assoc
   sort sort-by index-of includes? inc dec str + - * / < =`.

10. **One sequence view over every collection.** `count`/`empty?`/`first`/`rest`/`last`/
   `map`/`fold`/`reduce`/`into`/`vec`/`seq` walk a list, vector, `bytes`, a **set** (its
   elements) or a **map** (its `[k v]` pairs) — so no `(seq/zip (keys m) (vals m))`, and
   `(first {:a 1})` is `[:a 1]`. Map order is hash-driven, never insertion order.
   `conj`/`into` insert at each kind's natural point and **preserve the kind**;
   `conj`/`disj`/`get`/`contains?` on a set are prelude. Two deliberate exceptions:
   `contains?` is map/set only, and a **string is not seqable** — bridge with
   `string/->list` (codepoints) or `string/->graphemes` (what a human calls a
   character). `(sort coll)` is structural — `(sort [[1 0] [2 1]])` needs no comparator.

11. **`case` for constants, `match` for shapes.** `(case k :a 1 :b 2 default)` —
   flat `test result` pairs, lone trailing form is the default, tests must be
   **literals** (a bare symbol is an error: in `match` it would silently *bind*).
   A keyword is callable — `(:name p)`, `(map people :name)` — but nothing else
   data-like is (`({:a 1} :a)` is an error). `(comment …)` ignores its body (no `#_`).

12. **Patterns: `or`/`and`/map sub-patterns work; `not` and `:as` don't.**
   `(or 1 2)` matches either (every alternative must bind the same names);
   `(and whole {:keys [a]})` captures *and* destructures — that is Brood's `:as`;
   `{:k pat}` requires the key present and matches its value (while `{:keys [a]}`
   binds nil when absent). `(not …)` is an error (use a `:when` guard) and so is
   `:as` inside a map pattern (use `and`).

13. **Retired and never-were names.** `fn` only — no `lambda`. `car`→`first`,
   `cdr`→`rest`, `concat`→`append`, `length`→`count`, `some?`→`any?` (a pred over a
   coll; for non-nil write `(not (nil? x))`), `entries`→`%map-pairs`,
   `flat-map`→`mapcat`. Write `(not (= a b))`, not `not=` (deprecated — it warns, and
   `(not= 1 2 1)` is `true`). Check with `(bound? 'name)` if unsure.

14. **Never `def` a name Brood ships** — the prelude, builtins and embedded std
   modules are RESERVED, so `(defn map …)` / `(def get …)` are errors. Your own globals
   redefine freely (hot reload). A taken name is available three ways: a different
   name, a local `let` shadow, or a `(defmodule your/mod …)` where it becomes
   `your/mod/name`.

15. **A `sig` is a runtime contract under `nest run` / `nest test`.** Both arm
   contracts by default, so `(sig f (int -> int))` makes a call with a string raise
   `{:kind :contract :blame :caller …}`, and a wrong result blames the callee. A
   module's calls to its *own* functions skip the check (the contract guards the
   module boundary); a released binary never checks. So a sig that says *less* than the
   code does — `bool` where `nil` arrives, `vector` where a list is passed — breaks the
   tests. **Declare low, derive high:** the checker infers most signatures, so write a
   `sig` only where a leaf knows a fact nothing above it can prove, and never on a
   function whose type the checker already derives. Placement is free (above or below
   the `defn`).

## Naming & shape (match std/)

- `foo?` predicate · `*foo*` dynamic/module var · `foo->bar` conversion.
  Kebab-case. **Don't add a trailing `!`** — nothing mutates, so it warns of
  nothing (the few in-tree `!` names mean unrelated things: `sig!` = enforced
  in every mode, `(! pid msg)` = a `gen` cast).
- **Private = `defn-` / `def-`**, not a marker in the name. The name stays clean at
  the def and every call site; `(reflect/private? 'mod/name)` asks the image. The
  retired `--`-in-name convention still appears in old code — ignore it.
- Tail-recursive helpers: public shell delegates to a `defn-` private
  `name-acc`/`-loop` worker that carries the accumulator.
- Docstring (one-sentence summary, first line) on every public `defn`/`defmacro`;
  backticks/**bold**/`-` bullets render, and an indented `form → result` line is an
  executed example. Each module opens with `(defmodule name "…")`.
- Errors: `(error "fn-name: what went wrong: " value)` — lowercase, value appended.
- Three or more optional params → one options map destructured with `{:keys [...]}`.

**Modules are namespaces.** `(defmodule name …)` compiles the file into namespace
`name`: `def`/`defn` define `name/foo`. To call another module's names **bare**, add a
`(:use mod)` clause to the header (`(defmodule app "…" (:use editor/display) (:use
test))`); `(:use mod :only [a b])` for a subset. Merely *referencing* `mod/name` loads
the module on first use but leaves names qualified. The header accepts exactly
`(:use …)`, `(:use-internals …)`, `(:alias …)` and `(:load …)` — anything else,
`(:require …)` included, is an error. From outside a module (REPL, `nest mcp` eval)
reach a `defn` qualified: `(life/step …)`.

## Use the MCP server as your coding loop

A Brood project scaffolds `.mcp.json` pointing at `nest mcp` — a Model Context
Protocol server over the **live image**. When it's attached, prefer it over
guessing: it's how you check that the code you're about to write actually works.
Its tools:

- **`eval`** — evaluate a Brood expression in the running image. Use it to test a
  function before committing it to a file, or to reproduce a bug. `(io/puts …)` is
  safe — stdout is captured and returned as a separate block.
- **`load`** — load a file into the image (re-`def`s its globals, hot-reload).
  The image is a separate world from disk: after editing a file, `eval` sees the
  *old* defs until you `load` it. Reflex: edit → `load` → `eval`.
- **`lookup`** — a global's arglist, docstring, and source location. Check a
  function's real argument order here instead of assuming (trap #3).
- **`macroexpand`** — see what a macro expands to.
- **`format`** — format a snippet/file the way `nest format` would.
- **`callers`** — every reference to a global across the project (rename impact).
- **`apropos`** / **`all-globals`** / **`doc-search`** — does X exist, and what is it
  called?

## Don't guess the standard library

Probing for names one at a time (`rand`? `rand-int`? `random`?) burns
round-trips. Read the whole reference once instead: `nest doc --all` prints every
builtin and prelude fn/macro with its signature and summary; `nest doc <module>` does
the same for one module (`seq`, `string`, `math`, `editor/display`, …). What
muscle-memory reaches for, and what Brood actually has:

| You reach for | Brood has |
| --- | --- |
| `concat` | `append` — variadic over lists *and* vectors, returns a list |
| `loop`/`recur` | **neither exists** — a local loop is `(letrec (go (fn (i acc) … (go …))) (go 0 0))`; or a private tail-recursive helper, or `fold`/`reduce` |
| `(map f xs)`, `(reduce f init xs)` | `(map xs f)`, `(reduce xs init f)` — collection first (trap #3) |
| `str(...)` interpolation / printf | `(str a b)` concatenates; `(string/interp "a={a} b={(+ a 1)}")` interpolates (`{{`/`}}` are literal braces); `(string/format "%-8s|%6.2f" s x)` is printf (`%s %d %f %%`, width, `-` left-align) |
| `print` / `println` / `eprint` | `(io/puts x)` (line), `(io/write x)` (no newline), `(io/inspect x)` (re-readable); stderr is `(io/puts "boom" :to *err*)`. They space-join their args. No flush needed |
| `min` / `max` / `mod` / `abs` / `sqrt` | `math/min`, `math/max`, `math/mod`, `math/abs`, `math/sqrt`, `math/round`, `math/->fixed` (fixed decimals → string). Integer overflow promotes to bignum; `(/ 1 2)` is the ratio `1/2` |
| `parse-int` / `Integer.parseInt` | `(string/->number s)` → an int, a float, or a **failure** (trap #7); `(string/->number "1F" 16)` for a radix |
| `some?` (non-nil) | `(not (nil? x))`; Brood's `any?` is "some element matches a pred" |
| `set!` / `swap!` / atoms | nothing — state is a process or a `table` (trap #1) |
| `memoize`, `juxt`, `fnil`, `condp`, `if-some` | all exist, bare. `condp` is value-first: `(condp < 15 10 :small 100 :medium :large)` → `:medium` |
| `for` into a vector/map | `(for (x xs :when p :into []) …)`; an accumulating walk is `(fold-for (acc 0 x xs) (+ acc x))` |
| `pmap` / `take-while` transducer | `seq/pmap` (a process per item, order kept); `seq/transduce` with `seq/reduced` to stop early |
| raw ANSI (`clear`/`home`/cursor) | `(:use editor/ansi)` → `(ansi-clear)`/`(ansi-home)`/`(ansi-cursor r c)` are **zero-arg fns returning an escape string** — `(io/write (ansi-clear))`, never `(io/write ansi-clear)`. A render loop wants `editor/display` + `ui-run` |
| a built-in RNG (`rand`) | `rand/seed`, `rand/int`, `rand/float`, `rand/token`, plus `seq/shuffle`/`seq/sample` — pure & seedable: each takes a seed and returns `[value next-seed]`; thread the seed through your state |
| a set / `#{}` | first-class: `#{1 2 3}`, `(set? s)`, `(contains? s x)`, `(conj s x)`/`(disj s x)` with no import; `set/union`/`set/intersection`/`set/difference`/`set/subset?` for the algebra |
| `timeit` / `time` | `(dev/bench "label" expr)` prints `label: N ms` and returns the value |

## When to reach for a process (vs staying pure)

Pure functions are the default, but reach for a **process**
(`spawn`/`send`/`receive`) when you have:

- **Long-lived evolving state** — a counter, cache, or session. A process holding
  state in its `receive` loop is *the* way to express mutable state; the packaged
  form is a `gen` server (`defserver` + `gen/start`/`gen/call`/`gen/cast`).
- **CPU fan-out across cores** — `seq/pmap`, or `spawn` a worker per band and fan
  back in with `receive`. Measure the sequential version first; small inputs won't
  beat the spawn + copy-on-send overhead.
- **I/O multiplexing** — several blocking sources at once: one process per source,
  a coordinator `receive`s.

Otherwise **stay pure** — a tail loop or `fold`/`map` is simpler and easier to test.
`spawn` is a macro: `(spawn (work c))`, never `(spawn (fn () (work c)))` (the body
never runs). Messages **deep-copy** across per-process heaps, so a `send`-ed value is
independent in the receiver. Test concurrency with spawn-N-then-collect.

## When Brood crashes (a Rust panic)

A Rust-level panic — a *kernel* fault (use-after-GC tripwire, a heap index, a
runtime invariant), not your code raising — is appended to
**`.brood_crash_dump`** in the working directory: a `=== brood crash dump ===`
block with the timestamp, thread, the `panic: …` line, and a full backtrace.
The file is **append-only** — read the **last** block. It catches panics, **not**
`SIGSEGV`; deep recursion surfaces as a catchable `recursion too deep` error, not a
dump. A process that raises is reported by the default crash reporter (pid, reason,
trace) under `brood file` / `nest run`.

If Brood *itself* crashes (as opposed to your program erroring), that's a runtime
bug — write up a short report rather than working around it:

1. **Minimise** to the smallest `.blsp` that still reproduces, and make it
   deterministic.
2. **Capture** the last dump block, plus `brood --version`.
3. **Localise** with the knobs (`brood --debug-flags` lists them all); each one that
   flips the verdict (crash ↔ clean) names a subsystem:
   - `BROOD_GC_STRESS=1` — collect at every safepoint; makes a GC race deterministic.
   - `BROOD_GC_VERIFY=1` — walk the live graph before each collection; names the
     root→cell path of a stale handle.
   - `BROOD_TIER=1` (no JIT) / `BROOD_TIER=0` (tree-walker) — which execution tier.
   - `nest test --jobs 1` — serialise the scheduler to rule it in or out.
4. **File it** with the repro, the dump block and which knobs change the verdict.

## Before finishing

- Argument order is collection-first everywhere (trap #3); every qualified name you
  used exists (`nest check` reports an unbound one, and `nest run` refuses to start).
- Recursion in hot/iterative paths is **tail** recursion. No mutation crept in. No
  `[ ]` in a binding/param position. No bare symbol meant as a literal in a pattern.
- **`nest check`** is clean (advisory types, unbound names, non-tail recursion,
  constant conditions); `nest check --strict` also flags values merely wider than a
  parameter. Silence a *deliberate* finding with `(check-allow :category form…)` —
  e.g. `:non-tail-recursion`, `:discarded-catch`, `:constant-condition`.
- **`nest test`** passes — with contracts armed, so a `sig` that under-describes its
  function fails here (trap #15).
- **A TUI / animation loop is not covered by `nest test`** — run it bounded with
  `nest run --for 2s`; a full-screen TUI needs a real terminal, so wrap it in a pty —
  `script -qec "nest run --for 800ms" /dev/null 2>&1 | cat -v | grep -oE '\^\[\[[0-9;]*[A-Za-z]' | sort | uniq -c`.
- New public function has a docstring. Run `nest format` (whole-tree, **no file
  arg**). Keep `;` comments **out of vector/map literals and off `cond` clauses** —
  the formatter moves them; annotate above the form.
