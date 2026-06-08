# Brood language reference (v0.1)

This describes the language **as implemented today**. Anything not listed here
does not exist yet — see [roadmap.md](roadmap.md) for what's coming (dynamic
variables, a tracing GC, …).

Brood is a dynamically-typed, **immutable** **Lisp-1** (one namespace for
functions and variables, like Scheme/Clojure) with **lexical scoping** and
**proper tail calls**. The flavour is "clean and modern": code is made of lists
(so parameter lists are lists), `[...]` vectors are a data type, with
Clojure-style truthiness and `def`/`defn`/`fn`. Data never changes once made and
there is no local mutation — see [Immutability](#immutability).

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
| Multi-arity `(fn ((x) …) ((x y) …))` | **Supported** (ADR-047) — dispatch by argument count, like Clojure. But param lists are **lists** `(x)`, not vectors `[x]`, and a clause head may *also* be a same-arity **pattern** (Erlang-style; see [Pattern matching](#pattern-matching)). The two don't mix in one `defn`. | Vector heads `([x] …)` read as a one-tuple-param pattern clause, not an arity clause. |
| `{:a 1}` map literal | **Supported.** Immutable, insertion-ordered; `get`/`assoc`/`dissoc`/`keys`/`vals`/`contains?` (see [Maps](#maps)). | Works as you'd expect. |
| `{:keys [a b]}` / `:or` map destructuring | No map *patterns* yet (maps themselves exist; the pattern syntax for them doesn't). Sequence/tuple destructuring **is** supported — `(let ([a b] v) …)`, `(let ((h & t) v) …)`. | Parse / type error. |
| `(defn f [x y] …)`, `(let [a 1 b 2] …)` | Param lists and `let` bindings are **lists** — `(x y)` / `(a 1 b 2)`. | Works (vectors are accepted in binding position), but it's non-idiomatic — prefer lists. |
| `(/ 7 2)` → ratio `7/2` | No ratios. Integer args give an integer **only when they divide evenly**; otherwise a float. `(/ 12 3)` → `4`, `(/ 7 2)` → `3.5`. | A float where you expected an exact ratio. |

Within a *single* clause, optional and rest arguments use the Common-Lisp /
Emacs-Lisp spelling (`&optional`, `&`), described under
[Parameter lists](#parameter-lists). Brood *also* has Clojure-style multi-arity
(arg-count dispatch across clauses; ADR-047) — but the param lists are **lists**
`(x y)`, not vectors `[x y]`, and arity clauses don't mix with pattern/`&optional`
heads (see [`fn`/`defn` clauses](#fn--defn-clauses)). The list-not-vector spelling
is the one piece that can't be guessed from Clojure; it has to be read.

## Data types

| Type | Examples | Notes |
|---|---|---|
| Nil | `nil` | The empty value; also the empty list. |
| Boolean | `true`, `false` | |
| Integer | `0`, `42`, `-7` | 64-bit; arithmetic is overflow-checked. |
| Float | `3.14`, `-0.5`, `1e3` | 64-bit. |
| String | `"hello\n"` | Escapes: `\n \t \r \e \0 \\ \"` (`\e` is ESC, for ANSI terminal control), `\xHH` (two-hex-digit byte), `\u{H..H}` (1–6-hex-digit Unicode codepoint). A malformed `\x`/`\u{}` is a read error; any other `\X` is literal `X`. Readable printing is the inverse: it re-escapes `\n \t \r \e \0 \\ \"` by name and any other control char as `\u{H..H}`, so a printed string always re-reads to the same value. |
| Symbol | `foo`, `+`, `my-fn`, `empty?` | Names; interned. |
| Keyword | `:ok`, `:else` | Self-evaluating named constants. |
| List | `(1 2 3)`, `()`, `(1 . 2)` | Cons cells; `()` is `nil`. Quote to keep as data: `'(1 2 3)`. A dotted tail `(a . b)` makes an improper list (round-trips with the printer). |
| Vector | `[1 2 3]` | A data type with O(1) indexing. Evaluates its elements. |
| Map | `{:a 1 :b 2}`, `{}` | Immutable key→value associations; insertion-ordered. Evaluates its keys and values. Any value can be a key (compared structurally). |
| Function | `#<fn name>`, `#<native +>` | Closures and builtins. |
| Ref | `#<ref 0>` | A unique, opaque reference token from `(ref)` — no literal syntax; the only way to make one. Used to tag a request to its reply (see [Processes](#processes-concurrency)). |
| Pid | `#<pid a/7>` | A process id from `self`/`spawn`; carries node identity (`node/id`). No literal syntax. The location-transparent handle for `send` — local or across a node link (see [Distributed nodes](#distributed-nodes)). |

### Truthiness

Only `nil` and `false` are falsy. **Everything else is truthy**, including `0`,
`""`, and empty collections.

## Immutability

**Brood is an immutable language.** Once a value exists, nothing changes it; once
a binding is made, it never changes. Concretely:

- **Data is immutable.** There are **no data-mutation primitives** — no
  `set-car!`, `vector-set!`, `string-set!`, no atoms, refs, or cells. Operations
  like `cons`, `assoc`, `conj`, and `append` return a **fresh** value and leave
  their inputs untouched. Strings, lists, and vectors are read-only after
  construction.
- **Local bindings never change.** A `let` or `fn` binding is fixed for the life
  of its frame — there is no `set!` to rebind it.
- **The one mutation is `def`.** `def` rebinds a name in the **global**
  environment (even when written inside a function). This is *binding* mutation,
  not data mutation, and it exists for one reason: **live redefinition / hot
  reload** — the project's north star (ADR-013). A running process sees a `def`'d
  change on its next global lookup.
- **No imperative loop.** There is no `while` (and nothing to make it progress
  without mutation). Iteration is **recursion** — proper tail calls give O(1)
  stack — or, for state that must evolve over time, a **process** (`spawn` /
  `receive`) that carries the state through its own loop.
- **Mutable state, when truly needed, is never a mutable `Value`.** It takes one
  of two shapes: a **process** holding evolving state in its receive-loop (the
  Erlang model), or a **Rust-backed opaque resource handle** exposed through
  primitives (e.g. the coming rope/buffer, like a file handle) — mutation hidden
  behind the kernel, never aliasable Lisp data.

**Why it pays off.** Immutability removes the entire shared-mutable-aliasing bug
class and reinforces every other pillar of the system: the tracing GC needs no
write barriers or mutable roots; per-process heaps are trivially `Send` with
copy-on-send messages and no aliasing hazards; the shared `RUNTIME` code region
can be append-only; and it keeps the safe-Rust guardrail (ADR-001) honest. It
also shrinks the core — two fewer special forms (`set!`, `while`). See
[ADR-026](decisions.md) for the full rationale and trade-offs (e.g. repeated
immutable `assoc`/`append` is O(n²); `reduce`/`fold` and future persistent
structures are the mitigations).

## Maps

Maps are immutable key→value collections, written `{key value …}`:

```lisp
{:name "Ada" :born 1815}          ; a literal — evaluates keys and values
{}                                ; the empty map
(hash-map :a 1 :b 2)              ; built programmatically (same result as {:a 1 :b 2})
```

Like vectors, a map literal **evaluates** its keys and values, so
`{:sum (+ 1 2)}` is `{:sum 3}` and `{k 1}` uses the *value* of `k` as the key.
Any value can be a key — keywords, strings, numbers, even vectors or maps — and
keys are compared by **structural equality** (so `{[1 2] :v}` can be looked up
with `[1 2]`). Duplicate keys keep the **last** value. Maps preserve **insertion
order** when printed and when you ask for `keys`/`vals`. Map equality (`=`) is
**order-independent**: `{:a 1 :b 2}` equals `{:b 2 :a 1}`.

Maps are immutable — every operation returns a **fresh** map:

| Form | Meaning |
|---|---|
| `(get m k)` / `(get m k default)` | the value at `k`; `nil` (or `default`) if absent |
| `(assoc m k1 v1 k2 v2 …)` | a new map with the pairs added/updated (also works on a **vector** with integer indices — replaces, never appends) |
| `(dissoc m k1 k2 …)` | a new map with those keys removed |
| `(contains? m k)` | whether `k` is present (distinguishes a stored `nil` from absence) |
| `(keys m)` / `(vals m)` | the keys / values, as a list, in insertion order |
| `(reduce-kv f init m)` | fold over the entries: `(f acc k v)` left to right → the final acc |
| `(merge m1 m2 …)` | combine maps left to right; rightmost key wins (`nil` maps skipped) |
| `(merge-with f m1 m2 …)` | like `merge`, but a shared key's value is `(f old new)` |
| `(update m k f args…)` | a new map with `k`'s value replaced by `(f current args…)` (`current` is `nil` if absent; also works on a **vector** by integer index, which must be in range) |
| `(update-vals m f)` / `(update-keys m f)` | a new map with `f` applied to every value / key |
| `(select-keys m ks)` | a new map of just the entries whose key is in `ks` |
| `(zipmap ks vs)` | a map pairing `ks` with `vs` positionally (stops at the shorter) |
| `(get-in m path)` / `(get-in m path default)` | the value at a nested key `path`, or `default`/`nil` |
| `(assoc-in m path v)` | a nested copy with `v` stored at `path` (intermediate maps created) |
| `(update-in m path f args…)` | a nested copy with `path`'s value replaced by `(f current args…)` |
| `(count m)` / `(empty? m)` | number of entries / whether there are none |
| `(map? x)` | whether `x` is a map |

```lisp
(def person {:name "Ada" :born 1815})
(get person :name)                  ; => "Ada"
(get person :died "unknown")        ; => "unknown"
(assoc person :field "computing")   ; => a new map; `person` is unchanged
(-> person (assoc :born 1816) (get :born))   ; => 1816
```

These are thin Brood wrappers (`std/prelude.blsp`) over a small kernel of `map-*`
primitives; the internal representation is an insertion-ordered association
vector, which can be swapped for a hash-array-mapped trie later without any
surface change.

## Sets

There is no kernel set kind yet. A **set** is an opt-in library (`(:use set)` to
refer its names bare, `std/set.blsp`) built *on maps*: a set is a map of
`element → true`, so every map
operation already applies — membership is `(contains? s x)`, elements are
`(keys s)`, size is `(count s)`, and you can `fold`/`map`/`into` it. The module
adds only what maps lack: `(set coll)` (dedups), `conj`/`disj`, and the algebra
`union`/`intersection`/`difference`/`subset?`. Structural equality and vector
keys come from the map underneath, so `(set [[0 0] [1 2]])` is the natural
live-cell model for a grid. A first-class `#{…}` literal and a distinct `set?` are
deferred until they earn kernel support (reader/printer/`Value` variant) — see
the roadmap.

## Syntax

- `;` starts a line comment.
- `'expr` is shorthand for `(quote expr)`.
- Whitespace separates tokens; `[` `]` and `(` `)` delimit.
- A lone `.` inside a list builds a dotted (improper) tail: `(1 2 . 3)`. A `.`
  that begins an atom (`.5`, `.foo`) is not a separator.
- `{ }` is a map literal (`{key value …}`) — see [Maps](#maps). Commas count as
  whitespace, so `{:a 1, :b 2}` reads the same as `{:a 1 :b 2}`.

## Special forms

Special forms are evaluated specially (they don't evaluate all their arguments
eagerly). They are reserved names.

| Form | Meaning |
|---|---|
| `(quote x)` / `'x` | `x`, unevaluated. |
| `(if test then else?)` | Evaluate `then` if `test` is truthy, else `else` (or `nil`). |
| `(do body...)` | Evaluate forms in order; result is the last. |
| `(def name value)` | Define/redefine `name` in the **global** environment — redefinable, the language's only mutation. |
| `(fn (params) body...)` | A lexical closure. |
| `(let (a 1 b 2) body...)` | Sequential local bindings (each sees the previous). |
| `(letrec (f (fn ...) g (fn ...)) body...)` | Local **mutually recursive** bindings — every name is visible in every RHS (and to itself). Plain-symbol targets only; meant for fn definitions. |
| `` (quasiquote tmpl) `` / `` `tmpl `` | Template: literal except `~x` inserts a value and `~@xs` splices a sequence. |
| `(defmacro name (params) body...)` | Define a macro (see below). |

`when`, `unless`, `cond`, `and`, and `or` read like special forms but are
**prelude macros** over `if`/`do`/`let` (`std/prelude.blsp`), expanded once by the
compile pass (ADR-022) — so the evaluator's core stays minimal and they cost
nothing extra at runtime. `cond` is still flat test/expr pairs with `else`/`:else`
as the catch-all (ADR-004); `and`/`or` short-circuit left-to-right and return the
deciding value, each subexpression evaluated once. There is **no iteration special
form**: data is immutable and there is no local mutation (ADR-026), so loops are
expressed as recursion (proper tail calls make this O(1) stack) — or, for evolving
state, as processes (`spawn`/`receive`).

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

### Docstrings

A string literal as the **first body form** of a `fn`/`defn`/`defmacro` is a
**docstring** — *when more body follows it*. A function whose body is a lone
string returns that string (the CL/Elisp rule), so it isn't documentation:

```clojure
(defn square (x)
  "Return x times itself."   ; docstring (more body follows)
  (* x x))

(doc square)                 ;=> "Return x times itself."

(defn greeting (who) "hello") ; lone string → return value, NOT a docstring
(doc greeting)                ;=> nil
(greeting 'x)                 ;=> "hello"
```

The docstring is stored on the closure and read with `(doc f)` (below); it
powers editor hover / `describe-function` (see `docs/lsp.md`).

A **module** documents itself the same way: the docstring passed to its opening
`(defmodule name "…")` form (the file-level analogue of the function rule).
`nest doc <module>` renders both — the module docstring and every definition's
signature + docstring — as Markdown (see `docs/tooling.md`).

### Recursion is the loop

There is proper tail-call elimination, so recursion is the idiomatic way to
iterate and will not overflow the stack:

```clojure
(defn count-down (n)
  (when (> n 0)
    (count-down (- n 1))))
```

For purely side-effecting iteration, two prelude macros wrap the common patterns:

```clojure
(dotimes (i 3) (print i " "))    ; prints "0 1 2 "
(dolist (x (list :a :b))         ; runs the body for each element
  (println (name x)))            ; prints "a" then "b"
```

Both are tail-recursive and return `nil` (they're for effects). `doseq` (over
`for`) is the alternative when destructuring or `:when` filters are wanted.

Recursive **locals** — a helper fn that only exists inside one expression —
use `letrec`, which makes every binding name visible in every RHS:

```clojure
(letrec (even? (fn (n) (if (= n 0) true  (odd?  (- n 1))))
         odd?  (fn (n) (if (= n 0) false (even? (- n 1)))))
  (even? 10))                    ;=> true
```

Each RHS sees a placeholder `nil` for every name until its real value is
installed, so `letrec` is for mutually recursive **functions** (their bodies
fire at call time, by which point the real values are bound). For a one-shot
sequential binding, `let` is what you want.

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

### Auto-gensym (`x#`) — opt-in hygiene

Inside a backtick template, a symbol whose name ends in `#` (e.g. `tmp#`) expands
to a **fresh gensym**, the *same* one for every occurrence within that one
backtick expansion and a *distinct* one per expansion. This is the Clojure
shorthand for a non-capturing macro binding — a `tmp#` the template introduces can
neither capture nor be captured by the caller's `tmp`, with no manual `gensym`:

```clojure
(defmacro my-or (a b)
  `(let (r# ~a)            ; r# -> a fresh symbol, e.g. r__417
     (if r# r# ~b)))       ; same r__417

(let (r 1) (my-or false r))   ;=> 1  (the caller's `r` is not captured)
```

Auto-gensym fires only on *literal* template symbols; a `x#` inside an unquote
(`~(… x# …)`) is ordinary user code and is left alone. To emit a **literal**
`x#` (e.g. an anaphoric binding the caller is meant to see), unquote a quoted
symbol: `` `(let (~'it ~val) ~@body) ``. `gensym` itself remains available for
cases where you need a fresh symbol outside a template.

The `->` and `->>` threading macros are also defined in the prelude:

```clojure
(-> 5 (- 1) (* 2))            ;=> 8     ; (* (- 5 1) 2)
(->> (list 1 2 3) (map inc))  ;=> (2 3 4)
```

> Note: nested quasiquote is not level-tracked yet. Auto-gensym (`x#`) / `gensym`
> handle *binding* capture; *free* references in a macro template **auto-qualify**
> to the macro's defining namespace (ADR-066 α), so a macro expands correctly when
> used in another namespace without hand-qualifying. The advisory hygiene lint
> flags a plain literal binder that could capture a spliced argument. See spec §7.

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

`fn` is **multi-clause** when every form after it is a clause `(param-list body…)`.
Multi-clause dispatch has **two axes** (ADR-047):

- **By argument count** (Clojure-style multi-arity) when the heads are *arity
  clauses* — plain-symbol params, optionally with `&optional` / `&` rest. The
  call's arg count picks the clause; an exact fixed arity beats a variadic one,
  and among matches the most-specific (most required params) wins. Each arm binds
  its params *directly* (no rest-list), so it's as cheap as a single-clause fn —
  this is how the prelude's variadic `+`/`-`/`<`/`=` stay fast and stay Brood.
- **By pattern** (Erlang-style same-arity dispatch) when a head contains
  literals or destructuring — the clauses share an arity and the first matching
  shape (and `:when` guard) wins.

Otherwise `fn` is single-clause, and each **required** parameter may itself be a
pattern. `defn` inherits all of this (it forwards to `fn`).

```clojure
(defn greet                             ; multi-ARITY: dispatch by arg count
  ((name)          (greet name "hello"))
  ((name greeting) (str greeting ", " name)))
(greet "Ada")                           ;=> "hello, Ada"
(greet "Ada" "yo")                      ;=> "yo, Ada"

(defn count-args                        ; an arity arm may take & rest
  (()        0)
  ((a)       1)
  ((a & more) (+ 1 (length more))))

(defn fac                               ; multi-PATTERN: same arity, dispatch by shape
  ((0)  1)
  ((n)  (* n (fac (- n 1)))))

(defn area ([x y]) (* x y))             ; single-clause, tuple-destructured param
(defn move (p [dx dy] &optional (n 1))  ; patterns coexist with &optional / & rest
  …)
```

The two multi-clause axes **don't mix in one `defn`**: a head is read as *either*
an arity arm *or* a pattern clause. An `&optional`/`&` inside a clause that's being
matched as a pattern is treated as a literal symbol — it does *not* make that arm
variadic. Use arity overloading or pattern dispatch, not both in the same `defn`.

Parameter lists stay **lists** (ADR-010), so a single tuple parameter must be
wrapped: `(defn g ([x y]) …)` is one 2-tuple param, while `(defn g (x y) …)` is
two params.

**Matching and `&optional` don't nest.** `&optional` controls *arity*, patterns
control *shape*, multi-clause controls *dispatch* — and the three don't combine
into the optional slot:

- An `&optional` slot **must be a plain symbol** (with an optional default); it
  **cannot be a pattern**. `(defn k (x &optional ([a b] …)) …)` is a *type
  error* ("expected a symbol").
- **Don't mix `&optional` defaults / patterns with arity overloading.** A
  multi-clause `defn` is *either* arity-dispatched (every head is plain symbols,
  optionally with `&`/bare-`&optional`) *or* pattern-dispatched (some head carries
  a literal/destructuring/`(default …)` form, matched as a same-arity pattern). A
  head with a `(default …)` optional form is read as a *pattern* clause, so its
  `&optional` is matched literally and won't act as an arity marker. Overlapping
  arity arms that also use `&optional` are ambiguous — keep one mechanism per
  `defn`.
- Required parameters *can* still be patterns alongside `&optional` / `& rest`
  (only the optional/rest slots are restricted): `(defn move (p [dx dy]
  &optional (n 1)) …)` is fine.

To branch on an optional argument, **bind it as a symbol and `match`/`cond` on
it in the body** — using `nil` as the "was it omitted?" sentinel (or a custom
sentinel default like `(opt :none)` when `nil` is itself a legal value):

```clojure
(defn h (x &optional opt)
  (match opt
    (nil [:no x])        ; omitted → defaults to nil
    (v   [:yes x v])))
(h 1)                    ;=> [:no 1]
(h 1 2)                  ;=> [:yes 1 2]
```

**Idiom note.** The form `(defn area ([x y]) …)` is supported but **not
idiomatic** — it visually collides with multi-clause `(defn f ((p) body))`,
where the outer `(…)` wraps a clause. Prefer naming the param and unpacking
with `let`: `(defn area (p) (let ([x y] p) (* x y)))`. Multi-clause `defn`
pattern dispatch and tuple-destructured params on anonymous `fn` in
higher-order context (`(map (fn ([k v]) …) …)`) remain idiomatic. See
[brood-for-claude.md](brood-for-claude.md) §"Style — lists for code, vectors
for data" for the full rule.

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

`catch` binds `e` to the thrown value: a `throw` hands back its argument verbatim
(a bare string from `error`, a keyword, a `[:tag …]` vector, …), while a built-in
error (like division by zero) binds the kernel's canonical **error map** —
`{:kind :message [:code :file :line :col :hint]}` — so a handler can branch on
`(get e :kind)` without parsing strings. A `try` with no `catch` is just a `do`.
Under the hood `throw` and `%try` are primitives and `try`/`catch`/`error` are
written in Brood (`std/prelude.blsp`) — see [primitives.md](primitives.md).

Because a caught value has no single shape, **`(error-message e)`** is the
shape-agnostic accessor: a raised string as-is, the `:message` of an error map,
else the value's printed form. A `catch` handler that just wants a human string
uses it directly instead of branching on `string?`/`map?`:

```clojure
(try (risky) (catch e (log (str "failed: " (error-message e)))))
```

Type errors are **self-identifying**: they name the operation, the type it
wanted, and the tag + printed form of what actually arrived — e.g.
`type error: first: expected list or vector, got int (5)`. The tag word is the
[`type-of`](#predicates) name, so an error and `type-of` always agree.

## Dynamic variables

A **dynamic variable** is a global whose value can be temporarily overridden for
the *dynamic extent* of a body — the call tree it encloses — and then restored.
It's the Lisp "special variable", for config-style knobs (a print depth, a
current output sink) that a deep callee should read without threading the value
through every intermediate call.

```lisp
(defdyn *indent* 0)              ; declare a dynamic var with a default

(defn level () *indent*)         ; reads *indent* — whatever is bound right now

(level)                          ; => 0   (*indent* is its default)
(binding (*indent* 4) (level))   ; => 4   (rebound for this dynamic extent)
(level)                          ; => 0   (restored afterwards)
```

- **`(defdyn *name* default)`** declares `*name*` dynamic and gives it a default.
  The earmuffs (`*…*`) are convention, not syntax. Reading the var anywhere
  yields the default until a `binding` overrides it.
- **`(binding (*a* va *b* vb …) body…)`** evaluates the value expressions, binds
  each dynamic var for the duration of `body`, and **restores the previous values
  on exit — even if the body throws**. Bindings nest; the innermost wins. A
  reference resolves *dynamically*, at the moment it's evaluated, against the
  caller's bindings — not lexically where the function was defined.
- **`(dynamic? x)`** is true when `x` is a symbol declared with `defdyn`.

`binding` only accepts a variable previously declared with `defdyn`; rebinding an
undeclared global is an error (it's almost always a typo, and silently shadowing a
plain global would mislead). This is the one place a *binding* changes after it's
made — and like `def`, it's binding mutation, not data mutation; no value is ever
mutated (see [Immutability](#immutability)).

**`let` is always lexical, even for an earmuffed name.** `binding` is the *only*
form that creates a dynamic binding; a `let`/`fn` binding of a dynamic var's name
is an ordinary lexical binding that shadows it within that scope (this differs
from Common Lisp, where `let` on a special var binds dynamically — Brood follows
Clojure: lexical `let`, explicit `binding`). So `(let (*x* 5) (callee))` does
**not** change what `*x*` the callee reads, and a `let` that lexically binds `*x*`
will hide a `binding` of `*x*` inside its body. The rule: don't `let`-bind a
dynamic var's name — use `binding`.

**Dynamic bindings are per-process.** The binding stack lives in the process's
own heap, so a `binding` in one process is invisible to every other — and a
`spawn`ed child starts from the **defaults**, never inheriting the parent's
bindings (consistent with share-nothing: data isn't shared, and neither is
dynamic scope). If a child needs a value, send it explicitly. A process that
crashes mid-`binding` takes its binding stack down with it and disturbs no one.

`defdyn`/`binding` are Brood macros over a tiny kernel (`%declare-dynamic`,
`%binding`, `dynamic?`) — no new special form, the `try`/`catch` precedent.

## Output ports and logging

`print`/`println` don't write to stdout directly — they write to the **current
output port**. A *port* is just a one-argument function `(fn (s) …)` that consumes
a ready string; the dynamic variables `*out*` and `*err*` hold the current
stdout/stderr ports. The defaults write to the real streams (and `*out*` honours
the `with-out-str` capture), so out of the box `print` behaves exactly as you'd
expect. The point is that you can **redirect** it.

`(require 'io)` gives the port toolkit — constructors and the `with-out`/`with-err`
scoping macros (thin wrappers over `binding`):

```lisp
(require 'io)

(with-out (fn-port (fn (s) (collect s)))   ; route output to a callback
  (println "captured by collect"))

(with-out (process-port editor)            ; route output to another process …
  (println "sent as [:io-write \"…\\n\"]"))
```

A **`process-port`** sends each string to a process as `[:io-write s]`. That is
how output reaches a *buffer*: the process that owns the buffer (an editor's
`*Messages*`, say) receives the message and appends it. The string crosses the
process boundary as a copied message — async and share-nothing, never a mutated
value — which is exactly why it's safe. Because ports are plain functions, `print`
gains no special cases and `with-out-str` is unaffected. (Dynamic bindings don't
reach a `spawn`ed child, so a child starts with the default `*out*`; hand it a
port explicitly if it should redirect too.)

### Logging

`(require 'log)` is an **async, safe logger** built on the same idea. A logger is
one long-lived process (a `hatch` gen-server) holding a list of *backends*; each
log call is a fire-and-forget cast, so it never blocks the caller, and the single
process serialises every write — lines never interleave, and a backend that throws
takes down only that line, not the caller.

```lisp
(require 'log)

(start-logger)                          ; default: stdout, :info and up
(log-info "server up" {:port 8080})     ; structured fields are optional
(log :warn "disk low")
;; => [INFO  1736…] server up
;;    [WARN  1736…] disk low
```

Levels are `:debug` < `:info` < `:warn` < `:error`. A **backend** is an `io`
port + a minimum level + a formatter, so the logger *reuses* ports rather than
inventing its own sink. Build one with `stdout-backend` / `stderr-backend` /
`file-backend` / `fn-backend` / `process-backend`, and add it live:

```lisp
(add-backend (file-backend "app.log"))         ; also append to a file
(add-backend (process-backend buffer-pid))     ; …and to a buffer-owning process
```

`process-backend` is the **log-to-a-buffer** path: the formatted line is sent to
`buffer-pid` as `[:io-write s]` — the same envelope `process-port` uses — so an
editor process can fold it into its `*Messages*` buffer. The default logger is
registered under the name `:logger` (found via `whereis`); `(log …)` falls back to
stderr when none is running, so a log is never silently lost.

Both `io` and `log` are written in Brood over the process primitives — Rust only
supplies the render/write split behind `print` (`%render`, `%write-out`,
`%write-err`). See `std/io.blsp` and `std/log.blsp`.

## Type annotations

Types in Brood are **optional and advisory** — you never have to write one, and a
program with no annotations checks and runs exactly as before (see
[types.md](types.md) for the set-theoretic model). Two opt-in declaration forms
let you inform — and optionally *enforce* — the type system. Both are macros, not
special forms.

`(sig name (params… -> ret))` declares a function's signature. It is a pure
declaration — a runtime no-op — read by the advisory checker, which then flags a
provably wrong call against it (both the argument and the result type flow):

```clojure
(sig area (number -> number))
(defn area (r) (* 3.14159 r r))

(area "circle")           ; warning: area: argument 1 expects number, got string
(string-length (area 2))  ; warning: string-length: argument 1 expects string, got number
```

The type grammar: base names (`int float number string symbol keyword bool nil
pair vector list map fn any`), function arrows `(p… -> r)`, element-typed
sequences `(list E)` / `(vector E)`, and unions `(or A B …)`. An unrecognised
type-expression is ignored, never guessed.

`(sig! name (params… -> ret))` declares the **same** signature *and enforces it at
run time*: it wraps `name` so each argument and the result are checked on every
call, throwing on a mismatch (an opt-in "strong arrow"). Place it **after** the
definition — it rebinds the name, preserving arity.

```clojure
(defn area (r) (* 3.14159 r r))
(sig! area (number -> number))
(area "circle")   ;=> throws — area: argument 1 expected number, got string
```

`sig` is checker-only (zero runtime cost); `sig!` adds the runtime guarantee
exactly where you want soundness. The checker treats both identically. Writing a
*type* never changes behaviour; opting into *enforcement* (`sig!`) does. Full
design: [type-annotations.md](type-annotations.md) (ADR-082).

## Processes (concurrency)

Erlang-style **green processes**: cheap, lightweight, share-nothing (each runs
with its **own data heap**), communicating only by **message passing**. They run
on a small pool of worker threads (≈ one per core, or the CLI's `-j N`), so they
use every core; scheduling is **preemptively fair** — a CPU-bound process yields
its worker after a reduction budget, so one busy loop can't freeze the runtime.
Code is shared, data is not: a spawned function sees every `def` (and live
redefinitions — ADR-013), but messages cross as deep copies.

```clojure
(defn worker (parent)
  (let (n (receive))            ; suspend until a message arrives
    (send parent (* n 2))))     ; reply to the sender

(let (me (self))                ; capture the parent's pid *first* —
  (let (w (spawn (worker me)))  ; (self) *inside* spawn would be the child's pid
    (send w 21)
    (receive)))                 ;=> 42
```

`spawn` takes **one expression** and runs it in the new process — `(spawn (* (+ 1 1)))`,
`(spawn (worker me))`. The expression is *unevaluated*: it runs in the child, and its
free local variables are captured lexically (so `me` above crosses to the child like
any message). Because the body runs in the child, **`(self)` inside `spawn` is the
child's own pid** — to hand the parent's pid in, bind it in an enclosing `let` first
(the Erlang `Self = self(), spawn(fun() -> … end)` idiom).

| Form | Meaning |
|---|---|
| `(spawn expr)` | Run `expr` (unevaluated) in a new green process; returns its pid. Free locals are captured; `(self)` inside is the *child's* pid. |
| `(send target msg)` | Copy `msg` into `target`'s mailbox (non-blocking; a dead/unknown target is a no-op). `target` is a pid (local **or remote** — see [Distributed nodes](#distributed-nodes)) or a `{:name :node}` address. |
| `(receive clause...)` | Take the first matching message (see below); suspend until one arrives. `(receive)` with no clauses takes the next message. |
| `(self)` | Your own pid — a `:pid` value carrying this node's identity. |
| `(ref)` | A fresh unique reference token — see *Synchronous calls* below. |
| `(monitor pid)` | Watch `pid`; returns a monitor `ref`. See *Monitors* below. |
| `(demonitor mref)` | Drop the monitor created by `(monitor …)`. |
| `(exit pid reason)` | Send an exit signal to a local process (Erlang `exit/2`). `reason` `:kill` is the **untrappable** hard kill — the target dies at its next reduction tick, or immediately if parked, even in a tight loop. Any other `reason` is the **soft** signal — the target dies at its next `receive` (a tight non-`receive` loop won't honour it). Monitors fire `[:down ref pid reason]`. No-op for a dead/unknown pid. |
| `(spawn-count)` | How many green processes have been spawned since the program started. |
| `(peak-threads)` | High-water mark of processes running *simultaneously* (bounded by the worker pool). |
| `(worker-threads)` | Size of the worker-thread pool (≈ `nproc`, or `-j N`). |

### Selective receive

`receive` takes **pattern clauses** — the same grammar as `match`/`fn`
([Pattern matching](#pattern-matching)). It scans the mailbox in order, runs the
**first message that matches any clause**, and leaves non-matching messages
queued for a later `receive` (true Erlang selective receive — no head-of-line
blocking). Clauses may carry a `:when` guard.

```clojure
(receive
  ([:say text]      (println text))     ; clause = (pattern [:when guard] body...)
  ([:add a b]       (+ a b))
  (n :when (int? n) (handle-int n)))
```

An optional trailing **`(after ms body...)`** clause bounds the wait: if no
message matches within `ms` milliseconds, `body` runs instead. `(after 0 …)` is a
non-blocking poll. Because the timeout body is ordinary code, a timeout is
**catchable** — throw from it and catch with `try`/`catch` (Erlang's idiom):

```clojure
(try
  (receive ([:pong] :ok)
           (after 5000 (throw [:timeout])))   ; raise a structured, catchable value
  (catch e e))                                 ;=> [:timeout] on timeout
```

Messages are **copied** between processes. You can send a **closure** too: it
travels as data — its body is S-expression forms, its captured locals are copied,
and its free globals re-resolve on the receiver (so it runs on any node that has
the same definitions). This is what makes `(spawn expr)` shippable to another node.
A *builtin* can't be sent (it's a Rust function with no portable form) — reference
it by the symbol naming it instead, since code is shared. `receive` is a macro
over the `%receive` primitive, built on the `match` compiler — no new special
form. See [concurrency.md](concurrency.md) and [scheduler.md](scheduler.md) for
the model, and [pattern-matching.md](pattern-matching.md) for the clause grammar.

### Synchronous calls (and why there's no `await`)

`send` is fire-and-forget. To wait for a result, you don't need an `await`
primitive — the **blocking `receive` is the synchronisation**. The idiom is
Erlang's `gen_server` distinction: a *cast* is a bare `send`; a *call* is a
request whose reply you `receive`. The catch with concurrent calls is telling
replies apart, which is what **`(ref)`** is for: a fresh, opaque, unforgeable
token you put in the request and the server echoes in the reply, so a pinned
`~ref` in your `receive` matches only *your* answer (other replies stay queued).

```clojure
(defn reply (to tag v) (send to [:reply tag v]))
(defn call (pid req)
  (let (tag (ref))                       ; a unique token for this call
    (send pid [:call (self) tag req])
    (receive ([:reply ~tag v] v))))      ; block for exactly this reply
```

A script exits when its *main* process returns, so ending on a `call` (which
ends on a `receive`) is how you ensure spawned work finished before exit — no
separate `await`/join. `(ref)` values are their own type (`ref?`, `:ref`),
compared by identity, and may be sent in messages. (`call`/`reply` aren't in the
prelude yet — see `examples/life.blsp`.)

The opt-in **`task` module** (`(require 'task)`) packages the common "run this
thunk off my loop, with a timeout, cancellable" pattern over `spawn`/`receive`/
`exit`: `(task thunk opts)` returns a handle and delivers a tagged `[:task-done
handle v]` / `[:task-error handle msg]` / `[:task-timeout handle]` message to
`:reply-to`; `cancel-task` stops it early; and `(await thunk timeout-ms)` is the
*synchronous* run-with-timeout that blocks for the value (throwing on error or
timeout). This `await` is a userland convenience for bounding a single
computation — distinct from the gen_server `call` idiom above, which is the
right tool for request/reply to a long-lived process.

### The `proc/gen` server framework (gen_server in Brood)

`(require 'proc/gen)` packages the request/reply idiom above into a
gen_server-style framework — ~180 lines of Brood over `spawn`/`send`/`receive`/
`ref`/`monitor`, no kernel surface (ADR-099). A server carries one immutable
state value through a tail-recursive `receive` loop; `defprocess` declares how it
handles each kind of message:

```clojure
(defprocess counter (n)
  (init  (do (println "up") n))            ; runs once at startup; returns the initial state
  (cast  :inc            (+ n 1))          ; fire-and-forget; body = next state
  (call  :value          [n n])            ; synchronous; body = [reply next-state]
  (query :double         (* n 2))          ; synchronous read-only; body = the reply, state unchanged
  (info  [:down _ p r]   (do (log p r) n)) ; a non-envelope message (monitor/link/timer/raw send)
  (terminate reason (println "down: " reason)))  ; runs on (stop); body for cleanup

(def c (spawn-server counter 0))
(! c :inc)                 ; cast
(gen-call c :value)        ; => 1  (synchronous, 5 s default timeout)
(stop c)                   ; graceful shutdown — runs terminate, then ends the loop
```

The clause kinds map onto Erlang's `handle_cast`/`handle_call`/`handle_info` plus
two lifecycle hooks: **`cast`** (body → next state), **`call`** (body →
`[reply next-state]`; the caller blocks for the reply), **`query`** (a read-only
`call` — body → reply, state untouched), and **`info`** — a message that is *not*
a cast/call envelope: a monitor `[:down …]`, a link `[:EXIT …]`, a timer tick, or
a plain `send`. Optional **`init`** runs once at startup (the place to
`(trap-exit true)`, `(monitor …)`, arm a timer, or transform the seed) and
**`terminate`** runs on a clean `(stop pid)`. Envelope clauses are always matched
before `info` clauses, and **any message matched by no clause is dropped** rather
than left to pile up in the mailbox (OTP's default `handle_info`).

Client API: `(! pid payload)` casts; `(gen-call pid payload)` calls and blocks up
to 5 s (it `monitor`s the server, so a *dead* server raises at once instead of
hanging); `(gen-call-timeout pid payload ms)` sets a custom deadline; `(stop pid)`
ends the loop. Spawn with `spawn-server`, `spawn-server-link` (Erlang
`start_link` — links the server to the caller), or `spawn-server-named` (registers
it for `whereis`). A `defprocess` server composes directly under
`proc/supervisor` (see [supervision.md](supervision.md)).

### Monitors

`(monitor pid)` starts watching another process and returns a monitor `ref`.
When that process dies, the watcher receives one message:

```clojure
[:down <monitor-ref> <pid> <reason>]
```

`reason` is `:normal` for a clean return, `[:error <message>]` for a crash, and
`:noproc` if `pid` was *already* dead when you called `monitor` (the DOWN is then
delivered immediately). The monitor is **unidirectional** (it never affects the
watched process) and **one-shot** (it fires once). `(demonitor mref)` drops it,
best-effort — a DOWN already queued is not recalled. Pin the ref to wait for a
specific process's death and ignore unrelated messages:

```clojure
(def w (spawn worker))
(def m (monitor w))
(receive
  ([:down ~m _ :normal] :finished)
  ([:down ~m _ reason]   (restart reason)))   ; supervision, in-language
```

Monitors are the one kernel mechanism a **supervisor** is built from: watch your
children, and on a non-`:normal` DOWN, restart per a strategy — all expressible
in Brood. (Bidirectional `link`s are not implemented yet.)

### Distributed nodes

Two runtimes (separate OS processes) can **connect over TCP and message each
other** — *the network is just a longer copy*. A **pid carries node identity**, so
the same value addresses a process whether it's local or on a peer; `send` routes
transparently.

```clojure
;; node A: name the runtime, listen, expose a process by name
(node-start :a "127.0.0.1:9001" "secret")
(register :echo (self))

;; node B: connect, reach A's :echo by name, then talk to the pid it replies with
(node-start :b "127.0.0.1:9002" "secret")
(connect "a@127.0.0.1:9001")
(send {:name :echo :node :a} [:hi (self)])
(def peer (receive ([:pong p] p)))   ; p is a remote pid
(send peer [:ping (self)])           ; addressed directly — location-transparent
```

| Form | Meaning |
|---|---|
| `(node-start name "host:port" cookie)` | Name this runtime and listen for peers. Returns the node name. |
| `(connect "name@host:port")` | Dial + authenticate a peer (shared cookie). Returns the peer's node name. |
| `(register name pid)` | Bind a local name so peers can reach this process via `{:name name :node this-node}`. |
| `(node-name)` | This runtime's node name (`:nonode` until `node-start`). |
| `(nodes)` | A list of currently connected peer node names. |
| `(monitor-node name)` | Deliver `[:nodedown name]` when the link to `name` goes down (clean close or heartbeat timeout). |
| `(pid? x)` | True if `x` is a process id. |

The cookie is a shared secret (Erlang-style) — **not real security yet**. One node
per OS process. Remote `spawn`/code-shipping, distributed monitors, and node-down
detection are deferred. Full reference: [distribution.md](distribution.md).

## Builtins

> **Where these live:** only a small primitive kernel is implemented in Rust
> (the `%`-prefixed numeric ops, `cons`/`first`/`rest`, type predicates, I/O,
> `eval`/`load`, …). The functions below that aren't primitives — `+ - * / <
> = map filter reduce list …` — are defined *in Brood* in `std/prelude.blsp`,
> the same way you'd define your own. See spec.md §9 for the exact split. From a
> caller's point of view they're all just functions.

### Arithmetic
`+`  `-`  `*`  `/`  `mod`  `rem`  `quot`  `inc`  `dec`
`floor`  `ceil`  `round`  `round-to`  `sqrt`  `pow`  `abs`  `min`  `max`  `even?`  `odd?`

- Integer-only arguments give an integer result (`/` stays integer only when it
  divides evenly; otherwise it returns a float). Any float argument makes the
  result a float.
- `(- x)` negates; `(/ x)` is the reciprocal.
- Integer arithmetic is overflow-checked: an operation that would overflow
  (including `i64::MIN` cases like `(mod min -1)`) raises an error rather than
  wrapping or panicking. `(/ min -1)` falls through to a float.
- `rem` is the truncated remainder (sign of the dividend); `quot` is truncated
  integer division; `mod` is the euclidean remainder (always non-negative, in
  `[0, |b|)` — so `(mod 7 -3)` is `1`, not the floored `-2`).
- `floor`/`ceil`/`round` return an **int** (an int passes through unchanged);
  `round` rounds half away from zero. `round-to` keeps a fixed number of
  decimal *places* but stays a **number** (`(round-to 3.14159 2)` → `3.14`); for
  a fixed-width *string* like `"3.10"`, use `to-fixed` (under Strings). `pow` requires an **integer exponent**
  (use `sqrt` for roots): an int base with a non-negative exponent stays an int
  (overflow raises, like `*`); a negative exponent gives the reciprocal as a
  float. `sqrt` is always a **float** and is *approximate* — it's computed in
  Brood (Newton's method), not a hardware sqrt; redefine it if you need
  bit-exactness.
- `min`/`max` are variadic and require at least one argument. `even?`/`odd?`
  classify integers.
- Only `%add`/`%sub`/`%mul`/`%div`/`%lt`/`%eq`, `rem`, and `floor` are Rust
  primitives; **everything in this section is Brood** on top of them
  (`std/prelude.blsp`) — including `+`, `<`, and `=` themselves.

### Bitwise
`bit-and`  `bit-or`  `bit-xor`  `bit-not`  `bit-shift-left`  `bit-shift-right`

- Integer bit operations over the 64-bit two's-complement representation.
  `bit-and`/`bit-or`/`bit-xor` are binary; `bit-not` is the unary complement
  (`(bit-not n)` = `(- (- n) 1)`).
- `bit-shift-left` discards bits shifted past bit 63; `bit-shift-right` is an
  **arithmetic** (sign-preserving) shift. The shift amount must be in `[0, 64)`
  — outside that range is a clean error, not a crash.
- These are Rust primitives (they can't be bootstrapped from the numeric ops).

### Randomness
`rng`  `rand-seed`  `rand-int`  `rand-float`  `shuffle`  `sample`

- Brood has no global mutable state, so the PRNG is **pure and seedable**: every
  step takes a seed and returns `[value next-seed]`. Thread `next-seed` into the
  next call (carry it in your loop/process state like any other value). Seed a
  fresh stream from any integer — e.g. `(now)` — via `rand-seed`.
- `(rng seed)` → `[value next-seed]` with `value` a non-negative 32-bit int;
  `(rand-int seed n)` → `[i next-seed]`, `i` in `[0, n)`; `(rand-float seed)` →
  `[f next-seed]`, `f` in `[0.0, 1.0)`; `(shuffle seed coll)` →
  `[shuffled next-seed]`; `(sample seed coll)` → `[item next-seed]`.
- The generator is Marsaglia xorshift32 — fast, fine for simulations, sampling,
  shuffling, jitter, and ids; **not** for cryptography. All of it is Brood over
  the bitwise primitives (`std/prelude.blsp`).

### Comparison & logic
`=`  `not=`  `<`  `<=`  `>`  `>=`  `not`

- `=` is structural and variadic (`(= 1 1 1)` → `true`). Numbers compare within
  their type (`(= 1 1.0)` is `false`); use `<`/`>` for cross-type numeric order.
  Integers compare exactly (no precision loss past 2^53), and floats compare by
  IEEE value — so `(= 0.0 -0.0)` is `true` and `(= nan nan)` is `false`.

### Lists & sequences
`cons`  `first`  `rest`  `car`  `cdr`  `second`  `third`  `last`  `but-last`
`list`  `vector`  `append`  `concat`  `reverse`  `nth`  `count`  `length`  `empty?`
`range`  `take`  `drop`  `split-at`  `take-last`  `drop-last`  `take-while`  `drop-while`
`member?`  `some?`  `every?`  `find`  `index-of`  `index-where`  `zip`
`partition`  `sort`  `sort-by`  `subvec`  `remove`  `remove-nth`  `keep`
`distinct`  `dedupe`  `group-by`  `flatten`  `interpose`  `interleave`
`repeat`  `repeatedly`

- `first`/`rest` of `nil` are `nil`. `nth` takes an optional default:
  `(nth coll i default)`.
- `append` / `concat` (`concat` is an alias) concatenate any number of
  sequences — lists *and* vectors, read as sequences — left to right, returning
  a **list**; wrap in `(into [] …)` for a vector.
- `range`: `(range hi)` → `0..hi-1`; `(range lo hi)` → `lo..hi-1`;
  `(range lo hi step)` steps (ascending or descending). The result is a **lazy
  range** — an O(1) value that stands in for the list it denotes: it prints,
  compares (`=`), hashes, and `type-of`s exactly like that list, and
  `fold`/`reduce`/`sum`/`count` consume it in a counted loop with **zero
  allocation**; any other operation realises it to a real list on demand. An
  empty range is `nil`. `(range? x)` tests for the lazy handle (realised ranges
  are ordinary lists, so `range?` is false for them).
- `take`/`drop` clamp to the sequence length; `take-last`/`drop-last` take/drop
  from the end. `take-while`/`drop-while` split on the first element that fails
  the predicate. `split-at` returns `[front back]` — the first `n` items and the
  rest — in a single pass (the fused `take`+`drop`).
- `some?`/`every?` return booleans (`every?` is vacuously true on the empty
  list); `find` returns the first matching element, or `nil`.
- `index-of` returns the 0-based index of an element (by structural `=`), or -1;
  `index-where` is its predicate counterpart — the index of the first item for
  which `(pred x)` holds, or -1.
- `subvec` slices a vector, returning a **vector**: `(subvec v start)` to the end
  or `(subvec v start end)` for the half-open range `[start, end)` (the
  vector-preserving counterpart of `take`/`drop`, which return lists).
- `remove` is the complement of `filter`; `remove-nth` drops the element at a
  given index (returning a vector for a vector, a list for a list); `keep` maps a
  function and drops the `nil` results (map + filter fused).
- On a vector, `assoc`/`update`/`get` index by integer position — see
  [Maps](#maps) (`assoc`/`update`) and the index note there.
- `distinct` removes duplicates, keeping the first occurrence (order-preserving);
  `dedupe` collapses only *consecutive* runs of equal items.
- `group-by` buckets items into a map from `(f x)` to the list of items that
  produced it. `flatten` splices nested lists into one flat list (vectors/maps
  are leaves).
- `interpose` inserts a separator between adjacent items; `interleave` alternates
  two sequences, stopping at the shorter. `zip` pairs two sequences into `[x y]`
  vectors, stopping at the shorter. `partition` chunks into `n`-sized groups,
  dropping a trailing partial chunk.
- `repeat` builds a list of `n` copies of a value; `repeatedly` calls a
  zero-argument function `n` times and collects the results.
- `sort` orders ascending (or with a strict less-than predicate:
  `(sort > xs)`); `sort-by` orders by a key function. Both are a **stable**
  merge sort. All of these are tail-recursive (stack-safe on long inputs).

### Maps
`hash-map`  `get`  `assoc`  `dissoc`  `contains?`  `keys`  `vals`  `reduce-kv`
`merge`  `merge-with`  `update`  `update-vals`  `update-keys`  `select-keys`
`zipmap`  `get-in`  `assoc-in`  `update-in`  `map?`

See the [Maps](#maps) section above. `{ }` is the literal form; the rest are
immutable operations that return fresh maps. `count`/`empty?` work on maps too,
in **O(1)** — the CHAMP root node tracks its size (exposed by the `map-count`
kernel primitive), so neither walks nor materialises the entries.

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
`float?`  `bool?`  `fn?`  `vector?`  `map?`  `ref?`  `range?`

- `(type-of x)` returns the runtime type tag as a keyword — `:int` `:float`
  `:string` `:symbol` `:keyword` `:bool` `:nil` `:pair` `:vector` `:map` `:fn`
  `:macro` `:native` `:ref` — the spellings mirror the predicates above. It's the
  reflective primitive that in-language type checks build on; the predicates are
  the common-case shortcuts.

### Strings
`str`  `pr-str`  `string-length`  `substring`  `char-at`  `string->list`
`list->string`  `upper`  `lower`  `string->number`  `number->string`
`index-of`  `string-contains?`  `join`  `string-split`  `replace`
`trim`  `triml`  `trimr`  `blank?`  `starts-with?`  `ends-with?`
`string-repeat`  `pad-left`  `pad-right`  `to-fixed`  `format`

- `str` concatenates the *display* form of its args; `pr-str` returns the
  *readable* form of one value.
- There is **no distinct character type** (deferred): a "character" is just a
  1-char string, so `(char-at s i)` and the elements of `(string->list s)` are
  strings. All indices are **char-based**, matching `string-length` (so they are
  correct for multi-byte UTF-8, not byte offsets).
- `substring`, `char-at`, `string-length` are the char-indexed accessors;
  `string->list` / `list->string` bridge to and from a list of chars.
- `upper` / `lower` case-fold (Unicode-aware: `(upper "ß")` → `"SS"`).
- `string->number` is a **strict** parse — int if it is one, else float, else
  `nil`; it rejects partial input (`(string->number "3abc")` → `nil`) and
  surrounding whitespace (`trim` first if needed). `number->string` is its inverse
  (just `str` on a number).
- `index-of` returns the first char index of a substring or `-1`;
  `string-contains?` is the boolean form. `join` puts a separator between strings;
  `string-split` is its inverse (an empty separator splits into characters).
  `replace` swaps every occurrence of one substring for another.
- `trim` / `triml` / `trimr` strip whitespace (both ends / left / right);
  `blank?` is true for an empty or all-whitespace string.
- `string-repeat` concatenates n copies; `pad-left` / `pad-right` justify a
  string into a fixed-width field with spaces (never truncating). `to-fixed`
  renders a number with a fixed decimal count (`(to-fixed 3.14159 2)` → `"3.14"`)
  — the float→text op `str`/`pr-str` can't do, since they print the shortest
  round-tripping form. Together they handle tabular/console output. `to-fixed` is
  a Rust primitive (Rust's float formatter); the rest are Brood.
- `format` is a small `printf`-style wrapper: `(format "x = %d, y = %.2f" 42 3.14)`
  → `"x = 42, y = 3.14"`. Specifiers: `%s` (any, via `str`), `%d` (number),
  `%f` (float, 6 decimals), `%.Nf` (float, N decimals — uses `to-fixed`), `%%` (literal
  `%`). Width/justification isn't built in (compose with `pad-left`/`pad-right`).
  An unknown specifier or a truncated one errors; a missing arg renders as
  `nil`, extra args are ignored.

```clojure
(string-split "a,b,c" ",")      ;=> ("a" "b" "c")
(join "-" (list "x" "y" "z"))   ;=> "x-y-z"
(replace "one fish two fish" "fish" "cat")  ;=> "one cat two cat"
(upper (trim "  hi  "))         ;=> "HI"
(string->number "3.5")          ;=> 3.5
```

Only `upper`/`lower` (Unicode tables), `string->number` (strict parse-or-nil),
and `to-fixed` (float formatting) are Rust primitives; the rest of the library is
Brood over `substring`/`str` (`std/prelude.blsp`) — the "write the language in
the language" principle.

### I/O
`print`  `println`  `with-out-str`

- `print` writes the display forms of its arguments to stdout (space-separated);
  `println` adds a trailing newline. Both **flush stdout on every call**, so an
  animation frame paints immediately — there is no separate flush primitive (and
  none is needed).
- `(with-out-str body...)` evaluates `body` with stdout **captured** and returns
  everything it printed as a string (`""` if nothing), discarding `body`'s own
  value. Capture is process-scoped *and* inherited by any process `body` spawns,
  so a printer running in a child is captured too; and captures **nest** (the
  buffer is a stack), so a `with-out-str` inside another capture — e.g. a `nest
  mcp` tool handler, whose output is diverted off the JSON-RPC channel — drains
  only its own output. The buffer is released even if `body` throws (the error
  re-raises). Built on the `%capture-begin`/`%capture-take` kernel primitives.
- For simple raw-terminal control, `(:use editor/ansi)` provides escape *strings*
  to `print`: `ansi-clear` (erase + home — the per-frame reset), `ansi-cursor`,
  `ansi-home`, `ansi-hide-cursor`/`ansi-show-cursor`. The ESC byte is the `\e`
  string escape. For a structured render-op frame buffer instead, use
  `std/display` (`term-draw`/`term-emit`).

### Time & memory
`now`  `now-ns`  `bench`  `mem-bytes`  `mem-peak`

- `(now)` returns wall-clock milliseconds since the Unix epoch as an integer.
  Subtract two readings to measure elapsed time — the test runner uses it to
  report how long a suite took. `(now-ns)` is the same in **nanoseconds**, for
  timing work too fast for millisecond resolution (i64 ns stays in range until
  2262).
- `(bench "label" expr)` (a macro) evaluates `expr`, prints `label: N ms`, and
  returns `expr`'s value — drop it around any expression to time it in place.
- `(mem-bytes)` returns the bytes currently allocated process-wide, and
  `(mem-peak)` the high-water mark since the process started. They are fed by a
  byte-counting global allocator, so they cover *all* Rust allocations (the
  interpreter included), not just Brood values — which is what you want for
  "how much memory did this use." The test runner prints the peak alongside the
  time.
- `(gc-stats)` returns a snapshot map of this process's garbage collection —
  `{:collections :copied :reclaimed :live :live-bytes :threshold :debug-build}` —
  for observing reclamation (`:debug-build` is `true` when the binary carries debug
  assertions, i.e. *not* a performance build); `process-info` carries the
  per-process `:collections` count too. `(gc-collect)` forces a collection now and returns that same map
  (an observability/test aid, *not* a load-bearing trigger), and `(gc-trace on?)`
  toggles per-collection stderr logging for the calling process (no arg = query;
  defaulted from `BROOD_GC_TRACE`). **Memory is reclaimed automatically:** the
  LOCAL heap is a **generational** copying collector (a nursery every `alloc`
  bumps into, plus a tenured old generation) that fires at the eval safepoint
  (ADR-055) whenever a process's live set crosses an adaptive threshold — a minor
  collection copies the nursery's survivors and drops the rest, an occasional
  major compacts the old generation (ADR-072). So a long-running tail loop or
  `receive` server runs in bounded memory with nothing from the author — no
  manual GC call, no `while`, just recursion. (You never collect by hand; the old
  `(hibernate)` primitive that did so was removed once automatic collection
  landed.) The three thresholds are tunable for a given workload via
  `BROOD_GC_FLOOR` / `BROOD_GC_TENURE` / `BROOD_GC_MAJOR` (object counts, `K`/`M`
  suffixes accepted).

### Metaprogramming / self-hosting
`eval`  `read-string`  `read-all`  `eval-string`  `load`  `require`  `macroexpand`  `macroexpand-1`  `gensym`

`(require 'name)` loads an embedded standard-library module (e.g. `(require 'test)`
for the test framework) — works from any directory. It only *loads*: the module's
names stay qualified (`test/describe`). To refer them **bare**, put a `(:use name)`
clause in your `defmodule` header (see Namespaces) — that auto-loads too, so
`(:use test)` subsumes `(require 'test)`.

```clojure
(eval (read-string "(+ 40 2)"))  ;=> 42
(read-all "(a) (b) (c)")         ;=> ((a) (b) (c))  — every form, vs read-string's first
(eval-string "(def x 1) (+ x 1)");=> 2  — read+eval all forms, last value wins
(load "some-file.blsp")          ; evaluate a file into the global environment
```

`read-string` returns the *first* form in a string; `read-all` returns *all* of
them as a list (the read-half of `eval-string` without the eval) — the basis for
form-by-form tooling, e.g. an editor evaluating the last sexp before point. Both
raise on a malformed/incomplete form; `parse-source` is the lossless,
error-tolerant alternative (it yields a CST, used by the formatter).

These three are the seed of "edit the system while it runs": read code, evaluate
it into the live environment, replace definitions.

### Namespaces

A file opens a **namespace** with `(defmodule foo "optional doc")` as its first
form (one per file — `defmodule` *is* the namespace form; there is no separate
`ns`). Inside it, every `def`/`defn`/`defmacro` defines the **qualified** name
`foo/name`, and a bare reference resolves to `foo/name` when this namespace
defines it (including a *forward* reference to something defined later in the
file), otherwise it falls through to the **root** namespace — the prelude and any
non-namespaced globals. This keeps first-party and third-party code from
clobbering each other in the one shared global table (ADR-019/065), without a
separate namespace axis in the core: `foo/name` is just one interned symbol (`/`
is an ordinary symbol character), so the runtime, hot reload, and `send`/copy are
unchanged.

```clojure
(defmodule text "buffer text ops")
(defn insert (buf i s) …)        ; defines text/insert
(defn append (buf s) (insert buf (len buf) s))   ; bare `insert` → text/insert
(map insert bufs)                ; `map` → root/prelude (not text/map)

;; from elsewhere — fully-qualified, and still openly redefinable:
(text/insert b 0 "x")
(def text/insert (fn …))         ; advice / hot reload works
```

Import other namespaces' names with `(:use …)` clauses in the header. `(:use mod)`
refers all of `mod`'s public names bare; `(:use mod :refer [a b])` refers just
those. A bare reference resolves **current namespace → imports → root**, so an
own-namespace definition shadows an import. `:use` auto-loads the module (it never
*fetches* a package — declared deps only). A bare top-level `(require 'mod)` only
*loads* `mod` — its names stay qualified (`mod/foo`); use a `(:use mod)` clause to
refer them bare. `:use` is the **only** import clause: `(:require …)` is not a
`defmodule` clause and any non-`:use` form in the header is silently ignored.

```clojure
(defmodule editor "the editor core"
  (:use editor/buffer)                 ; refer buffer's public names bare
  (:use text :refer [insert]))  ; refer just text/insert as `insert`
(defn open (path) (insert (new-buffer) 0 (slurp path)))   ; insert → text/insert
```

**Earmuffed `*foo*` names are ambient** — by convention dynamic/config vars
(`*load-path*`, `defdyn` vars). They are **never** namespaced: a `(def *width* …)`
in any module defines root `*width*`, reachable bare everywhere (and so must be
project-unique). Every non-earmuff name is namespaced.

Privacy is **soft** (Clojure/CL-style, not Racket sealing): a `foo--internal`
name marked private by convention is skipped by `(:use)` refer-all but *still
reachable* by its qualified spelling, so live redefinition and advice keep working.
At the REPL the namespace tracks the last `defmodule`; `(current-ns)` reports it.

> Status: landed (ADR-065/066, 2026-05-30). `defmodule` is the single namespace
> form (`ns` removed); all of `std/` and every test file are namespaced; the
> checker, LSP, and `nest mcp` resolve names ns-aware. Macro templates
> **auto-qualify** their free references to the defining namespace (ADR-066 α), so
> a macro is robust across namespaces without hand-qualifying. Quoted symbols
> (`'foo`, message tags, map keys) are **never** qualified — they are data.
> Package-level name collisions are detected and rejected at dependency-resolution
> time (ADR-070), enforced once the package manager lands (ADR-037).

### Introspection (editor tooling)
`doc`  `arglist`  `global-names`  `bound?`  `all-globals`  `apropos`  `doc-search`

For self-description — the substrate an editor (and the planned language server,
`docs/lsp.md`) reads for hover, signature help, completion, and "is this name
known?". All derive from runtime state, so they stay correct as the program is
redefined.

```clojure
(defn add (a b & more) "Sum the arguments." (reduce + (+ a b) more))
(doc add)              ;=> "Sum the arguments."
(arglist add)          ;=> (a b & more)        ; mirrors the source surface
(bound? 'add)          ;=> true   (quote the name; bound? takes a symbol)
(bound? 'no-such)      ;=> false
(member? 'map (global-names))  ;=> true        ; every global, for completion
```

For **discovery** — finding what exists rather than describing a name you
already know (the answer to "is there an RNG?" in one call):

```clojure
(all-globals)            ;=> (… sorted list of every global …)  ; alias of global-names
(apropos "rand")         ;=> (rand-float rand-int rand-seed …)  ; names containing "rand"
(apropos :shuffle)       ;=> (shuffle shuffle--acc)             ; string/symbol/keyword pattern
(doc-search "random")    ;=> ([rand-int "…"] [sample "…"] …)    ; matches docstrings, not names
```

These three are Brood over `global-names`/`doc` (`std/prelude.blsp`), and are
also exposed as `nest mcp` tools (`apropos`, `all-globals`, `doc-search`) so an
agent can explore the live image — see `docs/mcp.md`.

## Prelude

`std/prelude.blsp` is loaded at startup and is where most of the language
actually lives — the `defn` macro, the arithmetic operators, comparisons,
equality, the sequence library, and the `->`/`->>` threading macros, all defined
in Brood on top of the Rust primitive kernel. It also adds `inc` `dec`
`identity` `second` `third` `zero?` `positive?` `negative?` `abs` `max` `min`
`even?` `odd?` `sum` `product`. Because it's ordinary Brood, any of it can be redefined at
runtime — and every function in it is defined with `defn`, exactly as you'd
define your own.

## Standard library (opt-in modules)

These modules are baked into the binary but **not** loaded at startup — use
`(require 'name)` or `(:use name)` in a `defmodule` header to load one.
Run `nest doc <module>` for the full API of any module.

| Module | `require` name | What it provides |
|--------|---------------|-----------------|
| `std/datetime.blsp` | `'datetime` | Gregorian calendar arithmetic: `date-new`, `date->unix`, `unix->date`, `date-add`, `date-diff`, `date-format`, `date-parse`, parse/format patterns |
| `std/encoding.blsp` | `'encoding` | Hex and Base64 encode/decode: `hex-encode`, `hex-decode`, `base64-encode`, `base64-decode` |
| `std/stats.blsp` | `'stats` | Descriptive statistics: `mean`, `median`, `variance`, `stddev`, `percentile`, `mode`, `frequencies`, `stats-min`, `stats-max` |
| `std/stream.blsp` | `'stream` | Process-based pull streams (lazy, I/O-friendly): sources (`stream-from-list`, `stream-range`, `stream-from-socket`), transformers (`stream-map`, `stream-filter`, `stream-chunk`, `stream-lines`), terminals (`stream-fold`, `stream-to-vector`, `stream-pipe`) |
| `std/url.blsp` | `'url` | URL encoding/parsing: `percent-encode`, `percent-decode`, `query-encode`, `query-decode`, `parse-url`, `build-url` |
| `std/csv.blsp` | `'csv` | CSV parse and emit: `csv-parse`, `csv-parse-maps`, `csv-emit`, `csv-emit-maps` |
| `std/uuid.blsp` | `'uuid` | UUID generation: `uuid-v4` (random), `uuid-v7` (time-ordered, RFC 9562), `uuid-nil`, `uuid?` |
| `std/template.blsp` | `'template` | `{{var}}` string templating: `render`, `render-all` |
| `std/queue.blsp` | `'queue` | Purely functional FIFO queue and min-priority queue |
| `std/multimap.blsp` | `'multimap` | Multi-valued map (one key → multiple values) |
| `std/hash.blsp` | `'hash` | `sha256`, `hmac-sha256` (RFC 2104), `hash-string` (djb2) |
| `std/diff.blsp` | `'diff` | LCS-based sequence diff: `diff-seq`, `diff-lines`, `diff-summary`, `diff-patch`, `diff-unified` |

The following modules are also opt-in and live under `std/net/` and `std/tool/`:

```clojure
(require 'net/tcp)    ; tcp-listen / tcp-connect / tcp-send / tcp-close … (thin wrapper over the net primitives)
(require 'net/http)   ; http-get / http-post / http-request / serve / stream-response
(require 'net/sse)    ; Server-Sent Events helpers
(require 'test)       ; describe / test / assert= / is — the test framework
(require 'format)     ; printf-style string formatting
(require 'json)       ; json-encode / json-decode
(require 'regex)      ; re-match / re-find / re-replace (thin wrapper over the regex engine)
(require 'set)        ; set operations over maps: set-new / set-add / set-member? / set-union …
(require 'fuzzy)      ; fuzzy string matching
(require 'log)        ; structured logging
(require 'task)       ; promise-style async tasks over processes
```
