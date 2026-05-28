# Brood — a quick reference for Claude

A pocket guide for writing `.blsp` (Brood Lisp) — what to do, what *not* to, and
the patterns that aren't shared with other Lisps. For depth see
`docs/language.md`, `docs/spec.md`, `docs/pattern-matching.md`,
`docs/concurrency.md`.

## What Brood is (and isn't)

A small, dynamic Lisp implemented in Rust.

- **Immutable data** (ADR-026). There's no `set!` / `setq`, no atoms, no cells
  — every operation returns a fresh value. The only mutation is `def`, which
  *re-binds* a global (hot reload). State that genuinely changes lives in a
  **process** (`spawn` / `send` / `receive`) or behind a Rust-backed handle.
- **No loops.** Use recursion (proper tail calls are guaranteed, including
  tail calls to *other* functions) or the higher-order combinators
  `fold` / `reduce` / `map` / `transduce`.
- **Truthy / falsy**: only `nil` and `false` are falsy. `0`, `""`, `()` are
  *truthy*.
- **Late binding**: globals can be re-defined; a redefinition is visible to
  every running process on its next lookup.

Files end `.blsp`. Run a file with `brood file.blsp`; REPL with bare `brood`;
project tooling via `nest test` / `nest run` / `nest new <name>`.

## Syntax

```
;; comment to end of line
42  -3  3.14            ; int (i64), float (f64)
"hello\n"               ; string
true  false  nil        ; booleans, nil
:keyword                ; keyword — interned, self-evaluating
name  foo-bar?  +       ; symbol (kebab-case is idiomatic)
(f a b)                 ; call / list
[1 2 3]                 ; vector — O(1) indexing
{:a 1 :b 2}             ; map — immutable, insertion-ordered (no commas)
'x   `(a ~b ~@xs)       ; quote / quasiquote / unquote / splice
```

## Special forms

Only these are *special*; everything else is a function or a macro:

```
def  defmacro  fn  lambda  quote  quasiquote  if  do
let  let*  letrec  defdyn  binding
```

Common macros (expanded once at the compile pass — runtime-free): `defn`,
`cond`, `when`, `unless`, `and`, `or`, `match`, `try` / `catch`, `->`, `->>`,
`receive`, `spawn`.

## Defining things

```lisp
(defn greet (name) (str "hello, " name))            ; defn = (def greet (fn (name) ...))
(defn add (& xs) (fold %add 0 xs))                  ; variadic via & rest
(defn opt-arg (x &optional (y 10)) (+ x y))         ; optionals with defaults

(defmacro when (test & body) `(if ~test (do ~@body) nil))

(def *flag* true)                                   ; global; def re-binds (hot reload)
(defdyn *log-level* :info)                          ; dynamic variable
(binding (*log-level* :debug) (do-thing))           ; scoped rebind
```

`fn` also supports multi-clause pattern dispatch:

```lisp
(defn classify
  ((0)               :zero)
  ((n) :when (< n 0)  :neg)
  ((n)               :pos))
```

Local bindings — `let` takes a **flat** name/value list (not Scheme's double-parens), and is sequential (each binding sees the earlier ones):

```lisp
(let (a 1
      b (+ a 1)         ; sees a
      [x y] some-vec)   ; destructuring works in the binding target
  (+ a b x y))
```

## Style — lists for code, vectors for data

Two rules that keep Brood code uniform and unambiguous. Both are about *idiom*;
both forms parse either way, but write the idiomatic one.

**1. Code uses `( )`; vectors `[ ]` are for data.** Param lists and the binding
forms of `let` / `for` / `doseq` / `when-let` / `if-let` are *lists*, not
Clojure-style vectors. Vectors are reserved for tuple values (`[x y]`),
sequence literals (`[1 2 3]`), and tuple **patterns** that match against tuple
values inside `match` / `let` / `receive` heads. Code is cons-lists so the
editor and macros manipulate one structure uniformly (ADR-010).

```lisp
;; good                          ;; not idiomatic
(let (a 1 b 2) …)                (let [a 1 b 2] …)
(for (x xs :when p) …)           (for [x xs :when p] …)
(doseq (x xs) …)                 (doseq [x xs] …)
(when-let (v (try-it)) …)        (when-let [v (try-it)] …)
```

**2. Don't tuple-destructure in a single-clause top-level `defn` param list.**
Name the param and unpack inside the body. Multi-clause `defn` (pattern
dispatch on clauses) is fine and encouraged — its clause heads use lists, not
vectors, so there's no ambiguity. Anonymous `fn` in higher-order context
(`map` / `reduce` / `mapcat`) **may** keep a tuple-destructured param — the
surrounding `(map …)` makes "this is a one-call function value" obvious, and
the alternative is a noisy extra `let`.

```lisp
;; good
(defn area (p) (let ([x y] p) (* x y)))

(defn neighbours (cell)
  (let ([x y] cell)
    (map (fn ([dx dy]) [(+ x dx) (+ y dy)]) offsets)))

;; multi-clause defn is fine — clause heads are lists, no [ ] collision
(defn fac
  ((0) 1)
  ((n) (* n (fac (- n 1)))))

;; not idiomatic — single-clause defn with a tuple-destructured param
(defn area ([x y]) (* x y))
(defn neighbours ([x y])
  (map (fn ([dx dy]) [(+ x dx) (+ y dy)]) offsets))
```

**Why rule 2:** `(defn f ([x y]) body)` is *single-clause* with one
tuple-destructured param, but visually collides with *multi-clause* `(defn f
((p) body))` where the outer `(…)` wraps a clause. The disambiguation is
correct (the parser checks whether the inner head is a list); the *reader*
pays a re-parse every time. The cost is highest at a top-level `defn` — that
name is the thing readers look up later. Confining the rule there preserves
the ergonomic `(map (fn ([k v]) …) …)` idiom, which reads locally and never
gets looked up by name.

## Patterns (`let`, `fn`, `match`, `receive`)

The trap: a bare symbol *binds*, it doesn't match. To match a known value,
pin it.

```
_                wildcard — matches anything, binds nothing
x                bind x; a repeated x is an equality constraint (non-linear)
42 "s" :k nil    literal match
'sym             match the symbol `sym`
~expr            pin — match the *current value* of `expr`
(p1 p2 ...)      list of exact length
(p1 & rest)      head(s) + tail
[p1 p2 ...]      vector of exact length (the tuple / tagged-data idiom)
```

```lisp
(match shape
  ([:circle r]    (* 3.14 r r))
  ([:rect w h]    (* w h))
  (_              0))
```

## Looping is recursion

```lisp
(defn sum-to (n acc)
  (if (= n 0) acc
    (sum-to (- n 1) (+ acc n))))         ; tail-recursive: O(1) stack
```

Prefer the higher-order combinators:

```lisp
(reduce + 0 xs)
(map sq xs)
(filter even? xs)
(fold (fn (m k) (assoc m k (* k k))) {} (range 10))
```

For longer pipelines, **transducers** fuse intermediate collections (one pass,
no throwaway lists):

```lisp
(transduce (comp (xmap sq) (xfilter even?)) + 0 (range 1000))
(transduce (xtake-while (fn (x) (< x 100))) + 0 (map sq (range 1000)))
```

## Concurrency — processes, not shared state

```lisp
(def me (self))
(spawn (send me [:reply 42]))                      ; child runs the expr
(receive ([:reply x] x))                           ; selective receive
```

Each process has its own heap; messages are **deep-copied** on `send`. `(self)`
is the current process's pid. Functions can't be sent (per-heap closures) —
send data and call `def`'d names on the receiving side. `receive` takes
pattern clauses just like `match`, plus an optional `(after ms body...)`
clause for timeouts.

## Errors

```lisp
(try
  (work)
  (catch e
    (println "failed: " e)))

(throw [:my-error :reason])              ; throwable values are arbitrary
(error "x out of range: " x)             ; convenience: throw with a built string
```

## Common builtins

- **list / seq**: `first` `rest` `cons` `list` `count` `empty?` `nth`
  `reverse` `map` `filter` `reduce` `fold` `append` `mapcat` `sort` `take`
  `drop` `range` `zip` `partition` `frequencies`
- **string**: `str` `pr-str` `string-length` `substring` `index-of`
  `string-split` `join` `replace` `trim` `upper` `lower` `number->string`
  `string->number` `starts-with?` `ends-with?`
- **map**: `assoc` `dissoc` `get` `keys` `vals` `contains?` `into`
- **types**: `type-of` plus the `?` predicates — `int?` `float?` `string?`
  `symbol?` `keyword?` `bool?` `nil?` `pair?` `vector?` `map?` `fn?` `ref?`
  `pid?`
- **arithmetic**: variadic `+ - * /`; comparison variadic chains
  `< > <= >= =`
- **I/O**: `print` `println` `slurp` `spit` `load` `eval-string` `read-string`
- **Filesystem (stat-class)**: `file-exists?` `dir?` `list-dir` `file-mtime`
- **processes**: `spawn` `send` `receive` `self` `ref` `monitor` `demonitor`
- **transducers**: `comp` `xmap` `xfilter` `xremove` `xkeep` `xmapcat`
  `xtake-while` `transduce` `reduced` `reduced?`

## Pitfalls when generating Brood code

- **No `setq` / `set!` / atoms.** State = a process, or re-bind a global with
  `def`.
- **No `while` / `for`.** Use recursion (TCO is guaranteed) or
  `fold` / `map` / `filter` / `reduce` / `transduce`.
- **Bare symbols in patterns *bind*.** Match a literal symbol with `'foo`;
  match a runtime value with `~expr`.
- **`=` is structural** and recursive — two unrelated structures that look the
  same compare equal.
- **Variadic operators**: `(+ a b c)` works. The fast 2-arg primitives, when
  you really need them, are `%add` `%sub` `%mul` `%div` `%lt` `%eq`.
- **No commas in maps**: `{:a 1 :b 2}` — spaces only.
- **`let` bindings are flat**: `(let (a 1 b 2) ...)`, not Scheme's `(let ((a 1) (b 2)) ...)`. Same for `let*` / `letrec` / `binding`.
- **`nil` is distinct from `false`** — `(nil? false)` is `false`,
  `(false? nil)` is `false`. Both are falsy, neither is the other.
- **Tail position matters**: deep *non*-tail recursion overflows the
  green-process stack. Use a tail-recursive helper with an accumulator.
- **Not Clojure**: no `defprotocol`, no transients, no `loop` / `recur`
  (just plain recursion), no namespaced names (the module system is flat).
- **Not Scheme / CL**: no `setq`, no `cond`-with-`t`-catch-all (use `else`
  or `:else`).

## Module skeleton (what `nest new` scaffolds)

```lisp
;; src/hello.blsp
(defmodule hello "A second module — main requires it and calls greeting.")

(defn greeting () "hello world")
```

```lisp
;; src/main.blsp
(defmodule main "The project's entry-point module (nest run -> main/main).")

(require 'hello)

(defn main ()
  "Entry point: print the project's greeting."
  (println (greeting)))
```

```lisp
;; tests/hello_test.blsp
(require 'test)

(describe "hello"
  (test "greeting works"   (assert= (greeting) "hello world"))
  (test "greeting is text" (is (string? (greeting)))))
```

`describe` groups tests; `test` defines one. `(assert= actual expected)` checks
structural equality with a diff on failure; `(is expr)` asserts truthy.

`nest test` runs each test in its own green process. `nest run` invokes the
`main/main` entry by default (override in `project.blsp` via `:main`).

## When in doubt

`std/prelude.blsp` is the canonical example of idiomatic Brood — almost
everything below the kernel is written there in the language itself; read it.
Deep references: `docs/language.md` (full reference), `docs/spec.md` (the
formal spec), `docs/pattern-matching.md` (the pattern grammar in detail).
