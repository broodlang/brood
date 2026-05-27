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

## Data types

| Type | Examples | Notes |
|---|---|---|
| Nil | `nil` | The empty value; also the empty list. |
| Boolean | `true`, `false` | |
| Integer | `0`, `42`, `-7` | 64-bit; arithmetic is overflow-checked. |
| Float | `3.14`, `-0.5`, `1e3` | 64-bit. |
| String | `"hello\n"` | Escapes: `\n \t \r \0 \\ \"`. |
| Symbol | `foo`, `+`, `my-fn`, `empty?` | Names; interned. |
| Keyword | `:ok`, `:else` | Self-evaluating named constants. |
| List | `(1 2 3)`, `()` | Cons cells; `()` is `nil`. Quote to keep as data: `'(1 2 3)`. |
| Vector | `[1 2 3]` | A data type with O(1) indexing. Evaluates its elements. |
| Function | `#<fn name>`, `#<native +>` | Closures and builtins. |

### Truthiness

Only `nil` and `false` are falsy. **Everything else is truthy**, including `0`,
`""`, and empty collections.

## Syntax

- `;` starts a line comment.
- `'expr` is shorthand for `(quote expr)`.
- Whitespace separates tokens; `[` `]` and `(` `)` delimit.
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
are written in Brood (`std/prelude.lisp`) — see [primitives.md](primitives.md).

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

Messages are **copied** between processes (data only — you can't send a
function). Today each process is backed by an OS thread, and a spawned function
sees only the prelude/builtins plus its arguments (shared user code is a planned
follow-up). See [concurrency.md](concurrency.md) for the model and limitations.

## Builtins

> **Where these live:** only a small primitive kernel is implemented in Rust
> (the `%`-prefixed numeric ops, `cons`/`first`/`rest`, type predicates, I/O,
> `eval`/`load`, …). The functions below that aren't primitives — `+ - * / <
> = map filter reduce list …` — are defined *in Brood* in `std/prelude.lisp`,
> the same way you'd define your own. See spec.md §9 for the exact split. From a
> caller's point of view they're all just functions.

### Arithmetic
`+`  `-`  `*`  `/`  `mod`  `rem`

- Integer-only arguments give an integer result (`/` stays integer only when it
  divides evenly; otherwise it returns a float). Any float argument makes the
  result a float.
- `(- x)` negates; `(/ x)` is the reciprocal.

### Comparison & logic
`=`  `not=`  `<`  `<=`  `>`  `>=`  `not`

- `=` is structural and variadic (`(= 1 1 1)` → `true`). Numbers compare within
  their type (`(= 1 1.0)` is `false`); use `<`/`>` for cross-type numeric order.

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
(load "some-file.lisp")          ; evaluate a file into the global environment
```

These three are the seed of "edit the system while it runs": read code, evaluate
it into the live environment, replace definitions.

## Prelude

`std/prelude.lisp` is loaded at startup and is where most of the language
actually lives — the `defn` macro, the arithmetic operators, comparisons,
equality, the sequence library, and the `->`/`->>` threading macros, all defined
in Brood on top of the Rust primitive kernel. It also adds `inc` `dec`
`identity` `second` `third` `zero?` `positive?` `negative?` `abs` `max` `min`
`sum` `product`. Because it's ordinary Brood, any of it can be redefined at
runtime — and every function in it is defined with `defn`, exactly as you'd
define your own.
