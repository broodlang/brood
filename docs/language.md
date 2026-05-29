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
| Multi-arity `(fn ([x] …) ([x y] …))` | No arity overloading. `fn` *is* multi-clause, but Erlang-style — **same-arity** dispatch by **pattern + guard** (see [Pattern matching](#pattern-matching)), not by arity. Use `&optional` / `&` rest for variable arity. | — |
| `{:a 1}` map literal | **Supported.** Immutable, insertion-ordered; `get`/`assoc`/`dissoc`/`keys`/`vals`/`contains?` (see [Maps](#maps)). | Works as you'd expect. |
| `{:keys [a b]}` / `:or` map destructuring | No map *patterns* yet (maps themselves exist; the pattern syntax for them doesn't). Sequence/tuple destructuring **is** supported — `(let ([a b] v) …)`, `(let ((h & t) v) …)`. | Parse / type error. |
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
| `(assoc m k1 v1 k2 v2 …)` | a new map with the pairs added/updated |
| `(dissoc m k1 k2 …)` | a new map with those keys removed |
| `(contains? m k)` | whether `k` is present (distinguishes a stored `nil` from absence) |
| `(keys m)` / `(vals m)` | the keys / values, as a list, in insertion order |
| `(reduce-kv f init m)` | fold over the entries: `(f acc k v)` left to right → the final acc |
| `(merge m1 m2 …)` | combine maps left to right; rightmost key wins (`nil` maps skipped) |
| `(merge-with f m1 m2 …)` | like `merge`, but a shared key's value is `(f old new)` |
| `(update m k f args…)` | a new map with `k`'s value replaced by `(f current args…)` (`current` is `nil` if absent) |
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
| `(fn (params) body...)` | A lexical closure. `lambda` is an alias. |
| `(let (a 1 b 2) body...)` | Sequential local bindings (each sees the previous). `let*` is an alias. |
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

A **module** documents itself the same way: a string literal as a file's
**first top-level form** is the module's docstring (the file-level analogue of
the function rule). Loading the file discards it harmlessly; the doc tooling
reads it from source. `nest doc <module>` renders both — the module docstring
and every definition's signature + docstring — as Markdown (see
`docs/tooling.md`).

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

`catch` binds `e` to the thrown value; for a built-in error (like division by
zero) it binds the error's message string. A `try` with no `catch` is just a
`do`. Under the hood `throw` and `%try` are primitives and `try`/`catch`/`error`
are written in Brood (`std/prelude.blsp`) — see [primitives.md](primitives.md).

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
  integer division; `mod` is the euclidean remainder (sign of the divisor).
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

### Comparison & logic
`=`  `not=`  `<`  `<=`  `>`  `>=`  `not`

- `=` is structural and variadic (`(= 1 1 1)` → `true`). Numbers compare within
  their type (`(= 1 1.0)` is `false`); use `<`/`>` for cross-type numeric order.
  Integers compare exactly (no precision loss past 2^53), and floats compare by
  IEEE value — so `(= 0.0 -0.0)` is `true` and `(= nan nan)` is `false`.

### Lists & sequences
`cons`  `first`  `rest`  `car`  `cdr`  `second`  `third`  `last`  `but-last`
`list`  `vector`  `append`  `reverse`  `nth`  `count`  `length`  `empty?`
`range`  `take`  `drop`  `take-last`  `drop-last`  `take-while`  `drop-while`
`member?`  `some?`  `every?`  `find`  `zip`  `partition`  `sort`  `sort-by`
`remove`  `keep`  `distinct`  `dedupe`  `group-by`  `flatten`  `interpose`
`interleave`  `repeat`  `repeatedly`

- `first`/`rest` of `nil` are `nil`. `nth` takes an optional default:
  `(nth coll i default)`.
- `range`: `(range hi)` → `0..hi-1`; `(range lo hi)` → `lo..hi-1`;
  `(range lo hi step)` steps (ascending or descending).
- `take`/`drop` clamp to the sequence length; `take-last`/`drop-last` take/drop
  from the end. `take-while`/`drop-while` split on the first element that fails
  the predicate.
- `some?`/`every?` return booleans (`every?` is vacuously true on the empty
  list); `find` returns the first matching element, or `nil`.
- `remove` is the complement of `filter`; `keep` maps a function and drops the
  `nil` results (map + filter fused).
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
immutable operations that return fresh maps. `count`/`empty?` work on maps too.

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
`float?`  `bool?`  `fn?`  `vector?`  `map?`  `ref?`

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
`print`  `println`

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

### Introspection (editor tooling)
`doc`  `arglist`  `global-names`  `bound?`

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

## Prelude

`std/prelude.blsp` is loaded at startup and is where most of the language
actually lives — the `defn` macro, the arithmetic operators, comparisons,
equality, the sequence library, and the `->`/`->>` threading macros, all defined
in Brood on top of the Rust primitive kernel. It also adds `inc` `dec`
`identity` `second` `third` `zero?` `positive?` `negative?` `abs` `max` `min`
`even?` `odd?` `sum` `product`. Because it's ordinary Brood, any of it can be redefined at
runtime — and every function in it is defined with `defn`, exactly as you'd
define your own.
