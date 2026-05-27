# Brood language reference (v0.1)

This describes the language **as implemented today**. Anything not listed here
does not exist yet — see [roadmap.md](roadmap.md) for what's coming (macros,
quasiquote, dynamic variables, error handling, maps, …).

Brood is a dynamically-typed **Lisp-1** (one namespace for functions and
variables, like Scheme/Clojure) with **lexical scoping** and **proper tail
calls**. The flavour is "clean and modern": code is made of lists (so parameter
lists are lists), `[...]` vectors are a data type, with Clojure-style
truthiness and `def`/`defn`/`fn`.

For the precise, normative version of everything here — grammar, evaluation
rules, scoping — see [spec.md](spec.md).

## Coming from Clojure (the differences that bite)

Brood's surface is deliberately Clojure-flavoured, so most Clojure reflexes
transfer unchanged: nil/false-only truthiness, type-sensitive `=`
(`(= 1 1.0)` is `false`), `:keyword`s, `cond` with flat test/expr pairs, the
`->`/`->>` threading macros, and quasiquote with `` ` `` / `~` / `~@` (Clojure's
choice, not Common Lisp's `,` / `,@`).

The catch is that a few core forms borrow from Scheme / Common Lisp instead, in
exactly the spots where a Clojure habit produces valid-looking code that fails
**silently or with a misleading error**. If you (or an LLM) write Clojure here,
these are the ones to unlearn:

| Clojure habit | Brood reality | What you get if you guess wrong |
|---|---|---|
| `(try … (catch Type e body))` | `catch` takes a **bare binding**: `(catch e body)`. There is no exception class. | The class name gets bound *as* the variable and `e` is treated as body → cryptic `unbound symbol: e`. |
| Multi-arity `(fn ([x] …) ([x y] …))` | No arity overloading. `fn` *is* multi-clause, but Erlang-style — **same-arity** dispatch by **pattern + guard** (see [Pattern matching](#pattern-matching)), not by arity. Use `&optional` / `&` rest for variable arity. | — |
| `{:a 1}` map literal | Maps aren't implemented yet. | `parse error: map literals '{ }' are not supported yet`. |
| `{:keys [a b]}` / `:or` map destructuring | No map patterns yet (maps aren't implemented). Sequence/tuple destructuring **is** supported — `(let ([a b] v) …)`, `(let ((h & t) v) …)`. | Parse / type error. |
| `(defn f [x y] …)`, `(let [a 1 b 2] …)` | Param lists and `let` bindings are **lists** — `(x y)` / `(a 1 b 2)`. | Works (vectors are accepted in binding position), but it's non-idiomatic — prefer lists. |
| `(/ 7 2)` → ratio `7/2` | No ratios. Integer args give an integer **only when they divide evenly**; otherwise a float. `(/ 12 3)` → `4`, `(/ 7 2)` → `3.5`. | A float where you expected an exact ratio. |

Optional and rest arguments use the Common-Lisp / Emacs-Lisp spelling
(`&optional`, `&`), described under [Parameter lists](#parameter-lists) — *not*
Clojure multi-arity. This is the one piece of the calling convention that can't
be guessed from Clojure; it has to be read.

## Data types

| Type | Examples | Notes |
|---|---|---|
| Nil | `nil` | The empty value; also the empty list. |
| Boolean | `true`, `false` | |
| Integer | `0`, `42`, `-7` | 64-bit; arithmetic is overflow-checked. |
| Float | `3.14`, `-0.5`, `1e3` | 64-bit. |
| String | `"hello\n"` | Escapes: `\n \t \r \e \0 \\ \"` (`\e` is ESC, for ANSI terminal control). |
| Symbol | `foo`, `+`, `my-fn`, `empty?` | Names; interned. |
| Keyword | `:ok`, `:else` | Self-evaluating named constants. |
| List | `(1 2 3)`, `()`, `(1 . 2)` | Cons cells; `()` is `nil`. Quote to keep as data: `'(1 2 3)`. A dotted tail `(a . b)` makes an improper list (round-trips with the printer). |
| Vector | `[1 2 3]` | A data type with O(1) indexing. Evaluates its elements. |
| Function | `#<fn name>`, `#<native +>` | Closures and builtins. |

### Truthiness

Only `nil` and `false` are falsy. **Everything else is truthy**, including `0`,
`""`, and empty collections.

## Syntax

- `;` starts a line comment.
- `'expr` is shorthand for `(quote expr)`.
- Whitespace separates tokens; `[` `]` and `(` `)` delimit.
- A lone `.` inside a list builds a dotted (improper) tail: `(1 2 . 3)`. A `.`
  that begins an atom (`.5`, `.foo`) is not a separator.
- `{ }` (maps) are reserved but not implemented yet — using them is a parse error.

## Special forms

Special forms are evaluated specially (they don't evaluate all their arguments
eagerly). They are reserved names.

| Form | Meaning |
|---|---|
| `(quote x)` / `'x` | `x`, unevaluated. |
| `(if test then else?)` | Evaluate `then` if `test` is truthy, else `else` (or `nil`). |
| `(when test body...)` | Evaluate `body` if `test` is truthy. |
| `(unless test body...)` | Evaluate `body` if `test` is falsy. |
| `(cond t1 e1 t2 e2 ...)` | Flat test/expr pairs (Clojure-style). `else` or `:else` always matches. |
| `(do body...)` | Evaluate forms in order; result is the last. |
| `(def name value)` | Define/redefine `name` in the **global** environment. |
| `(set! name value)` | Mutate the nearest existing binding of `name`. |
| `(fn (params) body...)` | A lexical closure. `lambda` is an alias. |
| `(let (a 1 b 2) body...)` | Sequential local bindings (each sees the previous). `let*` is an alias. |
| `(and a b ...)` | Left-to-right; returns the first falsy value, or the last. |
| `(or a b ...)` | Left-to-right; returns the first truthy value, or the last. |
| `(while test body...)` | Loop while `test` is truthy; returns `nil`. |
| `` (quasiquote tmpl) `` / `` `tmpl `` | Template: literal except `~x` inserts a value and `~@xs` splices a sequence. |
| `(defmacro name (params) body...)` | Define a macro (see below). |

### Parameter lists

Parameter lists are written as **lists** — `(defn f (x y) …)` — because code is
made of lists (vectors `[ ]` are a data type; they're still accepted in parameter
position, but lists are idiomatic). A list has three optional sections, in order:

```clojure
(defn add (a b) (+ a b))                 ; required
(add 2 3)                                ;=> 5

;; &optional: may be omitted; bare defaults to nil, or give a default expr.
(defn greet (name &optional (greeting "hello"))
  (str greeting ", " name))
(greet "Ada")                            ;=> "hello, Ada"
(greet "Ada" "yo")                       ;=> "yo, Ada"

;; a default may reference an earlier parameter (left-to-right)
(defn rect (w &optional (h w)) (* w h))
(rect 5)                                 ;=> 25

;; & rest: everything left over, as a list
(defn my-list (& xs) xs)
(my-list 1 2 3)                          ;=> (1 2 3)

;; closures capture lexically
(defn adder (a) (fn (b) (+ a b)))
((adder 10) 5)                           ;=> 15
```

Arity is strict: too few required args, or too many when there's no `& rest`, is
an error. Named (`&key`) arguments are designed but not in this version — see
spec §7.4.

### Recursion is the loop

There is proper tail-call elimination, so recursion is the idiomatic way to
iterate and will not overflow the stack:

```clojure
(defn count-down (n)
  (when (> n 0)
    (count-down (- n 1))))
```

## Macros

A macro receives its arguments **unevaluated** and returns a form that is then
evaluated in its place. Templates are written with quasiquote: `` `x `` quotes,
`~x` unquotes (inserts a value), `~@xs` splices a sequence.

```clojure
;; defn is itself a macro, defined in the prelude:
(defmacro defn (name params & body)
  `(def ~name (fn ~params ~@body)))

(defn square (x) (* x x))     ; => (def square (fn (x) (* x x)))

;; your own:
(defmacro unless2 (c & body) `(if ~c nil (do ~@body)))
(unless2 false (println "ran"))

;; inspect an expansion without running it:
(macroexpand-1 '(defn f (x) x))   ;=> (def f (fn (x) x))
```

`gensym` returns a fresh unique symbol for hygiene-by-convention. The `->` and
`->>` threading macros are also defined in the prelude:

```clojure
(-> 5 (- 1) (* 2))            ;=> 8     ; (* (- 5 1) 2)
(->> (list 1 2 3) (map inc))  ;=> (2 3 4)
```

> Note: nested quasiquote is not level-tracked yet, and macros are unhygienic
> (use `gensym`). See spec §7.

## Pattern matching

Erlang/Elixir-style pattern matching, with **one pattern grammar reused at every
binding site**: `match`, refutable `let`, and `fn`/`defn` clauses. The compiler
is written in Brood (`std/prelude.blsp`) — no new special form. For the full
design and rationale see [pattern-matching.md](pattern-matching.md).

### The grammar

| Pattern | Matches / binds |
|---|---|
| `_` | anything; binds nothing |
| `x` | anything; **binds** `x` (a repeated `x` is an equality constraint) |
| `42` `"s"` `:k` `true` `nil` | a literal, compared with `=` |
| `'sym` | the literal symbol `sym` |
| `~expr` | the current value of `expr` (a *pin*) |
| `(p1 p2 …)` | a list of that exact length, element-wise |
| `(p1 & rest)` | head(s) + the tail bound to `rest` |
| `[p1 p2 …]` | a vector of that exact length — the **tagged-data / tuple idiom** |

Patterns nest to any depth. **The one trap:** a bare symbol *binds* (and
shadows) — it does **not** test against a same-named value. Match a known value
with a keyword (`:ok`), a quoted symbol (`'none`), or a pin (`~x`).

### `match`

Clauses are **wrapped** `(pattern [:when guard] body…)`; the first whose pattern
(and guard) matches runs its body. `case` is just `match` with literal patterns.

```clojure
(match msg
  ([:say text]      (println text))
  ([:add a b]       (+ a b))
  ((x & xs)         (str "head " x ", rest " xs))
  (n :when (int? n) (handle-int n))
  (_                :unknown))          ; explicit catch-all
```

A `match` in tail position is TCO-safe (loops and receive loops won't overflow).
No clause matching **crashes** with a structured, catchable value
`[:match-error <context> <value> <patterns-tried>]` — add a `_` clause to make a
match total:

```clojure
(try (match resp ([:ok v] v))
  (catch e
    (match e
      ([:match-error ctx val pats] (recover val))
      (_                           (throw e)))))
```

### Refutable / destructuring `let`

A `let` binding target may be a pattern; it's a refutable bind (Brood's `=`) that
raises on mismatch. Bindings stay sequential, freely mixed with plain symbols:

```clojure
(let (a 1                    ; plain symbol (unchanged)
      [:ok v] (fetch key)    ; refutable: raises if fetch isn't [:ok _]
      (x & xs) (range 10))   ; destructure a list
  (use a v x xs))
```

### `fn` / `defn` clauses

`fn` is **multi-clause** when every form after it is a clause `(param-list body…)`
— Erlang-style same-arity dispatch by pattern (and guard). Otherwise it's
single-clause, and each **required** parameter may itself be a pattern. `defn`
inherits both (it forwards to `fn`).

```clojure
(defn fac
  ((0)  1)                              ; multi-clause dispatch
  ((n)  (* n (fac (- n 1)))))

(defn area ([x y]) (* x y))             ; single-clause, tuple-destructured param
(defn move (p [dx dy] &optional (n 1))  ; patterns coexist with &optional / & rest
  …)
```

Parameter lists stay **lists** (ADR-010), so a single tuple parameter must be
wrapped: `(defn g ([x y]) …)` is one 2-tuple param, while `(defn g (x y) …)` is
two params. Pattern destructuring of `&optional` slots is not supported yet.

## Errors

Raise with `throw` (any value) or `error` (a formatted message), and handle with
`try`/`catch`:

```clojure
(try
  (risky)
  (catch e
    (println "failed:" e)
    :recovered))

(throw :boom)                       ; raise an arbitrary value
(error "bad index: " i)             ; raise a message string
```

`catch` binds `e` to the thrown value; for a built-in error (like division by
zero) it binds the error's message string. A `try` with no `catch` is just a
`do`. Under the hood `throw` and `%try` are primitives and `try`/`catch`/`error`
are written in Brood (`std/prelude.blsp`) — see [primitives.md](primitives.md).

Type errors are **self-identifying**: they name the operation, the type it
wanted, and the tag + printed form of what actually arrived — e.g.
`type error: first: expected list or vector, got int (5)`. The tag word is the
[`type-of`](#predicates) name, so an error and `type-of` always agree.

## Processes (concurrency)

Erlang-style green-ish processes: each runs independently with its **own heap**
(share-nothing), and they communicate only by **message passing**.

```clojure
(defn worker (parent)
  (let (n (receive))          ; block until a message arrives
    (send parent (* n 2))))   ; reply to the sender

(def w (spawn worker (self))) ; start a process; (self) is our own pid
(send w 21)
(receive)                     ;=> 42
```

| Form | Meaning |
|---|---|
| `(spawn f arg...)` | Start a new process running `f` with the (copied) args; returns its pid. |
| `(send pid msg)` | Copy `msg` into `pid`'s mailbox (non-blocking; sending to a dead pid is a no-op). |
| `(receive)` | Take the next message from your own mailbox, blocking until one arrives. |
| `(self)` | Your own pid. |
| `(spawn-count)` | How many processes have been spawned since the program started (= worker OS threads created). |
| `(peak-threads)` | High-water mark of spawned threads running at once (bounded by the CLI's `-j N` concurrency cap). |

Messages are **copied** between processes (data only — you can't send a
function). Today each process is backed by an OS thread, and a spawned function
sees only the prelude/builtins plus its arguments (shared user code is a planned
follow-up). Because each process is one OS thread, `(spawn-count)` doubles as the
worker-thread count — the test runner uses it to report how much concurrency a
run used. See [concurrency.md](concurrency.md) for the model and limitations.

## Builtins

> **Where these live:** only a small primitive kernel is implemented in Rust
> (the `%`-prefixed numeric ops, `cons`/`first`/`rest`, type predicates, I/O,
> `eval`/`load`, …). The functions below that aren't primitives — `+ - * / <
> = map filter reduce list …` — are defined *in Brood* in `std/prelude.blsp`,
> the same way you'd define your own. See spec.md §9 for the exact split. From a
> caller's point of view they're all just functions.

### Arithmetic
`+`  `-`  `*`  `/`  `mod`  `rem`

- Integer-only arguments give an integer result (`/` stays integer only when it
  divides evenly; otherwise it returns a float). Any float argument makes the
  result a float.
- `(- x)` negates; `(/ x)` is the reciprocal.
- Integer arithmetic is overflow-checked: an operation that would overflow
  (including `i64::MIN` cases like `(mod min -1)`) raises an error rather than
  wrapping or panicking. `(/ min -1)` falls through to a float.

### Comparison & logic
`=`  `not=`  `<`  `<=`  `>`  `>=`  `not`

- `=` is structural and variadic (`(= 1 1 1)` → `true`). Numbers compare within
  their type (`(= 1 1.0)` is `false`); use `<`/`>` for cross-type numeric order.
  Integers compare exactly (no precision loss past 2^53), and floats compare by
  IEEE value — so `(= 0.0 -0.0)` is `true` and `(= nan nan)` is `false`.

### Lists & sequences
`cons`  `first`  `rest`  `car`  `cdr`  `list`  `vector`  `append`  `reverse`
`nth`  `count`  `length`  `empty?`

- `first`/`rest` of `nil` are `nil`. `nth` takes an optional default:
  `(nth coll i default)`.

### Higher-order
`map`  `filter`  `reduce`  `apply`

```clojure
(map inc (list 1 2 3))        ;=> (2 3 4)
(filter positive? (list -1 2 -3 4)) ;=> (2 4)
(reduce + 0 (list 1 2 3 4))   ;=> 10
(apply + (list 1 2 3))        ;=> 6
```

### Predicates
`nil?`  `pair?`  `list?`  `symbol?`  `keyword?`  `string?`  `number?`  `int?`
`float?`  `bool?`  `fn?`  `vector?`

- `(type-of x)` returns the runtime type tag as a keyword — `:int` `:float`
  `:string` `:symbol` `:keyword` `:bool` `:nil` `:pair` `:vector` `:fn`
  `:macro` `:native` — the spellings mirror the predicates above. It's the
  reflective primitive that in-language type checks build on; the predicates are
  the common-case shortcuts.

### Strings & I/O
`str`  `print`  `println`  `pr-str`

- `str` concatenates the *display* form of its args; `pr-str` returns the
  *readable* form of one value.

### Time & memory
`now`  `mem-bytes`  `mem-peak`

- `(now)` returns wall-clock milliseconds since the Unix epoch as an integer.
  Subtract two readings to measure elapsed time — the test runner uses it to
  report how long a suite took.
- `(mem-bytes)` returns the bytes currently allocated process-wide, and
  `(mem-peak)` the high-water mark since the process started. They are fed by a
  byte-counting global allocator, so they cover *all* Rust allocations (the
  interpreter included), not just Brood values — which is what you want for
  "how much memory did this use." The test runner prints the peak alongside the
  time. (Until the tracing GC lands, nothing is reclaimed mid-run, so the two
  read nearly the same.)

### Metaprogramming / self-hosting
`eval`  `read-string`  `load`  `require`  `macroexpand`  `macroexpand-1`  `gensym`

`(require 'name)` loads an embedded standard-library module (e.g. `(require 'test)`
for the test framework) — works from any directory.

```clojure
(eval (read-string "(+ 40 2)"))  ;=> 42
(load "some-file.blsp")          ; evaluate a file into the global environment
```

These three are the seed of "edit the system while it runs": read code, evaluate
it into the live environment, replace definitions.

## Prelude

`std/prelude.blsp` is loaded at startup and is where most of the language
actually lives — the `defn` macro, the arithmetic operators, comparisons,
equality, the sequence library, and the `->`/`->>` threading macros, all defined
in Brood on top of the Rust primitive kernel. It also adds `inc` `dec`
`identity` `second` `third` `zero?` `positive?` `negative?` `abs` `max` `min`
`sum` `product`. Because it's ordinary Brood, any of it can be redefined at
runtime — and every function in it is defined with `defn`, exactly as you'd
define your own.
