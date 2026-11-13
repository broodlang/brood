---
name: writing-brood
description: Use when writing, editing, debugging or testing Brood Lisp (`.blsp`) source — generating new code, changing a Brood project's modules or tests, or scaffolding a project. Brood is a small immutable Lisp implemented in Rust; it differs from Clojure/Scheme/CL in ways an LLM will get wrong by default (no mutation, no loops, collection-first argument order, lists-for-code/vectors-for-data, binding patterns, failures as values, runtime-enforced `sig`s). Load this before producing Brood code.
---

# Writing Brood

Brood (`.blsp`) is a small, dynamic, **immutable** Lisp. This page is the traps and
idioms you will get wrong if you write it like Clojure, Scheme or Common Lisp. The full
reference is `docs/brood-for-claude.md` (processes, servers, UI, hot reload, what to do
when the runtime crashes); `std/prelude/*.blsp` is the canonical idiomatic code.

## The traps

1. **No mutation.** No `set!`, `setq`, atoms, cells, `vector-set!`, `set-car!`; every
   operation returns a fresh value. The only mutation is `def` re-binding a **global**
   (hot reload). State that changes lives in a **process** (`spawn`/`send`/`receive`, or
   a `gen` server) or a `table`.

2. **No loops** — no `while`, `loop`/`recur`. Iterate with **tail recursion + an
   accumulator** (proper tail calls, O(1) stack) or `fold` / `reduce` / `map` /
   `seq/filter`. A local loop is a `letrec` closure called by name:
   `(letrec (go (fn (i acc) … (go …))) (go 0 0))`. Deep *non*-tail recursion raises a
   catchable `recursion too deep` (`E0044`). `for` exists, but it is a comprehension.

3. **Collection FIRST, function LAST.** `(map xs f)`, `(fold xs init f)`,
   `(reduce xs init f)`, `(seq/filter xs pred)`, `(seq/sort-by xs key-fn)` — the reverse
   of Clojure. Predicates too: `(string/starts-with? s "#")`, `(contains? m k)`,
   `(index-of coll x)`. A callback's own parameters keep the familiar order:
   `(fold xs 0 (fn (acc x) (+ acc x)))`.

4. **Lists for code, vectors for data.** Binding forms and parameter lists are
   **lists**: `(let (a 1 b 2) …)` (flat, not `((a 1) (b 2))`), `(for (x xs :when p) …)`,
   `(when-let (v e) …)`, `(defn f (a b) …)`. A vector there is an error. Vectors are
   data: tuples `[x y]`, literals `[1 2 3]`, tuple **patterns**, name lists that are
   data (`{:keys [a b]}`, `:sealed [a b]`).

5. **A bare symbol in a pattern BINDS.** In `match` / `fn` / `receive` / destructuring
   `let`, `x` binds; pin a known value with **`^x`** (not `~x`), a literal symbol with
   `'sym`, a constant with `42`/`:k`/`"s"`.

6. **Truthiness:** only `nil` and `false` are falsy, so `0`, `""`, `[]`, `{}` are truthy —
   but the empty **list** is `nil`; test with `(empty? x)`. `cond`'s catch-all is `else`
   (never `:else`/`t`/`true`). `index-of` answers **-1** when absent, so
   `(when (index-of s "x") …)` is always true: use `(includes? s "x")`.

7. **A known failure is a returned VALUE.** `(string/->number "abc")`, the decoders and
   `datetime/parse-*` return a `failure`, which is **truthy** — `(or (string/->number s) 0)`
   yields the failure. Test with `(failure? x)`, read with `(error-message x)`, chain with
   `ok->` / `with`. Bugs raise: `try`/`catch`, where `(catch pattern body…)` takes a pattern
   (`(catch e …)` catches all), never `(catch Type e …)`.

8. **Name a top-level `defn`'s param, unpack in the body:**
   `(defn area (p) (let ([x y] p) (* x y)))`. An anonymous `fn` may destructure:
   `(map m (fn ([k v]) …))`.

9. **Many familiar names live in a module and are unbound bare.** Output: `io/puts`,
   `io/write`, `io/inspect` (no `print`). `math/…`: `min max quot rem mod floor abs sqrt
   even? odd?`. `seq/…`: `zip subvec frequencies group-by find distinct filter reject
   sort-by partition take-while repeatedly`. `map/…`: `get-in assoc-in update-in`.
   `proc/sleep`, `os/now`; `reflect/…`: `gensym macroexpand doc apropos`; `debug/spy`;
   all strings are `string/…` (`string/join`, `string/format`, `string/interp "x={x}"`).
   Referencing `mod/name` loads the module; there is no `require` form. Bare: `map fold
   reduce count first rest conj into get assoc sort index-of includes? inc dec str + - * /`.

10. **One sequence view over every collection.** `count first rest last map fold reduce
   into vec seq` walk lists, vectors, `bytes`, sets and maps (as `[k v]` pairs:
   `(first {:a 1})` is `[:a 1]`; map order is hash order). `conj`/`into` keep the kind.
   Exceptions: `contains?` is map/set only, and a **string is not a sequence** — bridge
   with `string/->list` (codepoints) or `string/->graphemes`. `(sort coll)` is structural.

11. **`case` for constants, `match` for shapes.** `(case k :a 1 :b 2 default)` — literal
   tests, a lone trailing default. A keyword is callable (`(:name p)`, `(map people
   :name)`), a map is not. `(comment …)` ignores its body (no `#_`).

12. **Patterns:** `(or 1 2)` matches either (same names bound); `(and whole {:keys [a]})`
   is Brood's `:as`; `{:k pat}` requires the key. `(not …)` and `:as` are errors — use a
   `:when` guard and `and`.

13. **Retired names.** `fn`, not `lambda`; `first`/`rest`, not `car`/`cdr`; `append`, not
   `concat`; `count`, not `length` (a string's too: there is no `string/length`); `any?` (a pred over a coll), not `some?` — for non-nil
   write `(not (nil? x))`; `mapcat`, not `flat-map`; `(not (= a b))`, not `not=`
   (deprecated). Unsure? `(bound? 'name)`.

14. **Never `def` a name Brood ships** (prelude, builtins, std modules are reserved). Use
   another name, a local `let` shadow, or your own `(defmodule …)`.

15. **A `sig` is a runtime contract under `nest run` / `nest test`.** A wrong argument
   raises `{:kind :contract :blame :caller …}`; a wrong result blames the callee. A sig
   that says *less* than the code does (`bool` where `nil` arrives) fails the tests.
   **Declare low, derive high:** the checker infers most types, so write a `sig` only on a
   leaf that knows a fact nothing above it can prove.

## Naming and modules

- `foo?` predicate · `*foo*` dynamic var · `foo->bar` conversion · kebab-case · no
  trailing `!` (nothing mutates). Private is `defn-` / `def-`
  (`(reflect/private? 'mod/name)` asks).
- A public shell delegates to a private tail-recursive worker carrying the accumulator.
- A one-sentence docstring on every public `defn`/`defmacro`; an indented
  `form ;=> result` line in it is an executed example.
- Errors: `(error "fn-name: what went wrong: " value)`. Three or more optional params →
  one options map with `{:keys [...]}`.
- `(defmodule name "doc" (:use other) (:use test))` compiles the file into namespace
  `name`. `(:use mod :only [a b])` / `:exclude [a]` pick names. The header takes only
  `:use`, `:use-internals`, `:alias`, `:load`, `:shadow`, `:implements`. From outside a
  module, call it qualified: `(life/step …)`.

## Muscle memory vs Brood

| You reach for | Brood has |
| --- | --- |
| `concat` | `append` — variadic over lists and vectors, returns a list |
| `loop`/`recur` | a `letrec` local loop, a private tail-recursive helper, or `fold`/`reduce` |
| `(map f xs)` | `(map xs f)` — collection first (trap 3) |
| interpolation / printf | `(string/interp "a={a} b={(+ a 1)}")`; `(string/format "%-8s|%6.2f" s x)` |
| `print` / `println` | `(io/puts x)`, `(io/write x)`, `(io/inspect x)`; stderr `(io/puts x :to *err*)` |
| `min`/`max`/`mod`/`abs` | `math/min` … `math/round`, `math/->fixed`; ints promote to bignum; `(/ 1 2)` is `1/2` |
| `parse-int` | `(string/->number s)` → number or **failure**; `(string/->number "1F" 16)` |
| `set!` / atoms | a process or a `table` (trap 1) |
| `juxt`, `fnil`, `condp`, `if-some` | exist; the first two in `combinator/`, with `combinator/complement` and `combinator/constantly`; `comp`, `partial` bare |
| `for` into a collection | `(for (x xs :when p :into []) …)`; `(fold-for (acc 0 x xs) (+ acc x))` |
| `pmap` / transducers | `seq/pmap`; `seq/transduce` with `seq/reduced` |
| ANSI escapes | `(:use editor/ansi)` → `(io/write (ansi-clear))` — zero-arg fns returning strings |
| `rand` | pure, seeded: `rand/seed`, `rand/int`, `rand/float`, `rand/rng`, `seq/shuffle`, `seq/sample` return `[value next-seed]`; `(rand/token n)` is OS-random hex |
| sets | `#{1 2 3}`, `contains?`, `conj`, `set/disj`, `set/union` … |
| `time` | `(dev/bench "label" expr)` |

Don't probe names one at a time: `nest doc --all` prints every builtin and prelude
function; `nest doc seq` one module.

## Ask the project with `nest eval`, never a probe file

To learn what an expression does in this project — a function's answer, a value's shape,
whether a name is bound — evaluate it. Don't write a test file to ask.

```bash
nest eval '(seq/sort-by ["bb" "a" "ccc"] count)'   # the value, printed re-readable
nest eval --in my/module '(helper 3)'              # inside a module: its private names and imports resolve bare
nest eval --test '(test/scratch-dir)'              # as a test sees it: test/running? true, a scratch home
nest eval - <<'EOF'                                # several forms from stdin, no shell quoting
(def words (string/split "a b c" " "))
(count words)
EOF
```

- The project loads as `nest repl` loads it, in about half a second even for a big project.
  Contracts are armed, as under `nest test`.
- What the code prints comes first. The last line is the value of the last form, `pr-str`'d.
  An error is reported on stderr, with exit 1.
- Use **`--test`** when the code could write state (config, history, anything under the XDG
  directories). Without it the code runs against the real home.
- **Never write a throwaway `tests/zz_probe_test.blsp`, or any scratch file in the tree, to
  ask a question.** Other sessions and tools read the tree while you work. A file that appears
  and vanishes breaks their `nest check` and `nest test` runs, and it can be committed by
  accident. `nest eval` leaves nothing behind.

## The MCP server is your coding loop

A Brood project's `.mcp.json` points at `nest mcp`, a server over the **live image**
(tools appear as `mcp__brood__eval` and so on). Prefer it to guessing:

- **`eval`** runs source in the image; **`write`** / **`edit`** change a file and load and
  check a `.blsp` in the same call; **`load`** re-`def`s a file changed any other way (the
  image does not see disk edits until then).
- **`check`** (project) / **`check-source`** (a snippet); **`run-tests`**.
- **`lookup`** — a global's arglist, docstring, type and location; check argument order
  here. **`apropos`** / **`doc-search`** / **`all-globals`** — does it exist?
  **`find-pattern`** — the idiom for a task (`"loop"`, `"mutable state"`). **`error`** —
  what a code like `E0030` means.
- **`macroexpand`**, **`format`**, **`callers`**, **`bench`**, **`abilities`** /
  **`ability`**, and runtime views **`processes`** / **`process-info`** / **`node`** /
  **`vitals`** / **`watch-runtime`**.

A root `mcp.blsp` can add tools: `(defn mcp-tools () (cons {:name … :handler …} (mcp/tools)))`.

## Before finishing

- Argument order is collection-first; every qualified name exists; hot loops are tail
  recursive; no `[ ]` in a binding position; no bare symbol meant as a literal pattern.
- **`nest check`** is clean (`--strict` also flags values wider than a parameter); silence
  a deliberate finding with `(check-allow :category form…)`.
- **`nest test`** passes, with contracts armed (trap 15).
- A TUI or animation loop isn't covered by tests: run it bounded with `nest run --for 2s`
  (inside `script -qec "…" /dev/null` for a full-screen one).
- `nest format` (whole tree, no file argument). Keep `;` comments out of vector/map
  literals and off `cond` clauses — the formatter moves them.
