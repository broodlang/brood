# mylisp language reference (v0.1)

This describes the language **as implemented today**. Anything not listed here
does not exist yet — see [roadmap.md](roadmap.md) for what's coming (macros,
quasiquote, dynamic variables, error handling, maps, …).

mylisp is a dynamically-typed Lisp with **lexical scoping** and **proper tail
calls**. The flavour is "clean and modern, leaning Clojure-ish": `[...]` for
vectors and parameter lists, Clojure-style truthiness, and `def`/`fn`.

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
| Vector | `[1 2 3]` | Evaluates its elements. Also the shape of a parameter list. |
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
| `(fn [params] body...)` | A lexical closure. `lambda` is an alias. |
| `(let [a 1 b 2] body...)` | Sequential local bindings (each sees the previous). `let*` is an alias. |
| `(and a b ...)` | Left-to-right; returns the first falsy value, or the last. |
| `(or a b ...)` | Left-to-right; returns the first truthy value, or the last. |
| `(while test body...)` | Loop while `test` is truthy; returns `nil`. |

### Functions and `&` rest args

```clojure
(def add (fn [a b] (+ a b)))
(add 2 3)                  ;=> 5

;; variadic: everything after & is bound as a list
(def my-list (fn [& xs] xs))
(my-list 1 2 3)            ;=> (1 2 3)

;; closures capture lexically
(def adder (fn [a] (fn [b] (+ a b))))
((adder 10) 5)            ;=> 15
```

### Recursion is the loop

There is proper tail-call elimination, so recursion is the idiomatic way to
iterate and will not overflow the stack:

```clojure
(def count-down
  (fn [n]
    (when (> n 0)
      (count-down (- n 1)))))
```

## Builtins

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

### Metaprogramming / self-hosting
`eval`  `read-string`  `load`

```clojure
(eval (read-string "(+ 40 2)"))  ;=> 42
(load "some-file.lisp")          ; evaluate a file into the global environment
```

These three are the seed of "edit the system while it runs": read code, evaluate
it into the live environment, replace definitions.

## Prelude

`std/prelude.lisp` is loaded at startup and adds helpers written in mylisp
itself: `inc` `dec` `identity` `second` `third` `zero?` `positive?` `negative?`
`abs` `max` `min` `sum` `product`.
