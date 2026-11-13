# Brood language reference

This describes the language **as implemented today** (v0.50.0). Anything not listed here
does not exist yet — see [ROADMAP.md](../ROADMAP.md) for what's still open.

Brood is a dynamically-typed, **immutable** **Lisp-1** (one namespace for
functions and variables, like Scheme/Clojure) with **lexical scoping** and
**proper tail calls**. The flavour is "clean and modern": code is made of lists
(so parameter lists are lists), `[...]` vectors are a data type, with
Clojure-style truthiness and `def`/`defn`/`fn`. Data never changes once made and
there is no local mutation — see [Immutability](#immutability).

For the precise, normative version of everything here — grammar, evaluation
rules, scoping — see [spec.md](spec.md).

## Coming from Clojure (the differences that bite)

Brood is its own language, not a Clojure dialect. Its surface will look familiar to a
Clojure reader, and many reflexes carry over: nil/false-only truthiness, type-sensitive
`=` (`(= 1 1.0)` is `false`), `:keyword`s, `cond` with flat test/expr pairs, the `->`
threading macro, and quasiquote with `` ` `` / `~` / `~@` (not Common Lisp's `,` / `,@`).

Where Brood chose differently, the choice is deliberate, and each row below says why or
links the decision. They are listed because a Clojure habit in those spots produces
valid-looking code that fails **silently or with a misleading error**. If you (or an LLM)
write Clojure here, these are the ones to unlearn:

| Clojure habit | Brood reality | What you get if you guess wrong |
|---|---|---|
| `(try … (catch Type e body))` | `catch` takes a **pattern**: `(catch e body)` catches everything, `(catch {:kind :type} body)` one kind. There is no exception class. | A clean error naming the fix. (It used to bind the *class name* as the variable and evaluate `e` as a statement — and since the prelude then defined `e` (it is `math/e` now), `(catch Exception e (io/puts "caught" e))` silently printed 2.718…, a wrong program with no diagnostic.) |
| Multi-arity `(fn ((x) …) ((x y) …))` | **Supported** — but param lists are **lists** `(x)`, not vectors `[x]`, and clauses run **in order**: the first that matches runs, by argument count and by shape alike, so arity and [pattern](#pattern-matching) clauses mix (ADR-619). A clause an earlier one shadows is refused. | A **clean error** with a hint: vector-headed clauses `([x] …) ([x y] …)` are rejected. (They used to read as one 2-parameter pattern clause with an empty body — a completely different function, diagnosed only later as a misleading arity error at the call site.) |
| `{:a 1}` map literal | **Supported.** Immutable; `get`/`assoc`/`dissoc`/`keys`/`vals`/`contains?` (see [Maps](#maps)). Iteration order is **hash-derived, not insertion order** (unlike Clojure's array-map — see [Maps](#maps)). | Works as you'd expect, except don't rely on key order. |
| `{:keys [a b]}` / `:or` map destructuring | **Supported** — a map literal in pattern position binds each `:keys` symbol to the same-named keyword's value (nil if absent, or the `:or` default): `(let ({:keys [a b] :or {b 0}} m) …)`, works in `let`/`fn`/`match`. `{:key subpattern}` requires the key present and matches its value. There is no `:as`: write `(and m {:keys [a]})`. | `:as` in a map pattern is an error naming `and`. |
| `(defn f [x y] …)`, `(let [a 1 b 2] …)` | Param lists and `let` bindings are **lists** — `(x y)` / `(a 1 b 2)`. A vector *inside* one is still destructuring: `(let ([x y] p) …)`. | A clean error with a hint (ADR-149). The vector spelling was once accepted as an alias; that is what turned every Clojure binding shape into a silent misread — `(let [[a 1] [b 2]] …)` destructured `[a 1]` against `[b 2]` and reported `unbound symbol: b`. |
| `(defprotocol P (area [this]))`, `(extend-type T P (area [c] …))` | `defability` and `impl`, and their parameter lists are **lists** like every other: `(defability Shape (area (self)))`, `(impl Shape circle (area (c) …))` (ADR-512). A vector stays data — `defmethod`'s dispatch tuple `[money money]`, `:sealed [circle rect]`. | A clean error whose hint is the list spelling: `write (area (c) …), not [c]`. |
| `(let [[a 1] [b 2]] …)` / Scheme's `(let ((a 1)) …)` | Bindings are **flat**: `(let (a 1 b 2) …)`. | A clean error **with a hint** to flatten (was accepted-then-confusing). |
| `^:kw` / `^{:doc "…"}` metadata | Brood has **no metadata**. `^` is the pattern **pin**: `^expr` matches the current value of `expr` (ADR-150). | `^:kw` reads as a pin of the keyword — a pattern where you meant an annotation. Docstrings go in the body (see [Docstrings](#docstrings)). |
| `#{1 2 3}` set literal | A first-class set (`Value::Set`, ADR-060): `set?` true, prints `#{…}`, never `=` to a map. Evaluates its elements and dedups. | Reads as a kernel set; the `set` library (`(:use set)`) adds `union`/`intersection`/… |
| `#(+ 1 %)` anonymous-fn reader macro | Write it out: `(fn (x) (+ 1 x))`. | A parse error **with a hint** naming `(fn …)` (was "unbound symbol: #"). |
| `#'foo` var-quote | Symbols are values — plain `'foo`. | A parse error **with a hint** naming `'foo`. |
| `#_form` discard reader macro | No form-level discard — wrap the form in `(comment …)`, whose body is read but never evaluated, or comment it out with `;`. | A parse error **with a hint** naming `(comment …)` and `;` (was a stray `#_` symbol). |
| `#"[0-9]+"` regex literal | Regexes are library values: a `(regex/match? s "pat")` reference auto-loads the `regex` module. | A parse error **with a hint** naming `(regex/match? …)` (was a stray `#`-symbol). |
| `\c` / `\newline` character literal | No character type — a character is a 1-char string `"c"` (or `(string/int->char 99)`). | A parse error **with a hint** naming the 1-char string (was `unbound symbol: \c`). |
| `#\|…\|#` block comment (Scheme/CL) | No block comments — comment each line with `;`, or wrap forms in `(comment …)` (read but never evaluated). | A parse error **with a hint** (ADR-169; used to read as a bar-quoted symbol). Any other `#…` is likewise reserved — `#` is a dispatch character, and `#{…}` / `#b"…"` are its only forms. |
| `0x1F`/`0b1010`/`0o17` radix literals | **Yes** (ADR-334) — they read as plain ints: `0xFF` → `255`, `0b1010` → `10`, `0o17` → `15`, signed forms too. The radix is spelling, not a type — `(type-of 0xFF)` is `:int` and it prints back as `255`. Past `i64` it is a bignum, like an oversized decimal literal. | Malformed digits (`0b102`, `0xZZ`, a bare `0x`) are a parse error **with a per-radix hint**, never a symbol. |
| `1_000` separators, `1N` bigint | Neither — a digit-led token must be a number Brood has. `1000` needs no separator; plain `1` already widens to bignum. | A parse error **with a targeted hint** (ADR-169; these read as symbols before, surfacing as a far-away "unbound symbol"). Reserving the tokens is what let ratios (ADR-196) and radix literals (ADR-334) ship additively; the same holds for whatever comes next. |
| `(/ 7 2)` → ratio `7/2` | **Yes — this is what happens** (ADR-196). `/` on integers is exact: `(/ 7 2)` → `7/2`, `(/ 12 3)` → `4` (divides evenly → int). `1/2` is a reader literal. Reach for `->float` when you want an inexact result. | A ratio, exactly as in Clojure/Scheme. |
| `(map f coll)`, `(reduce f init coll)`, `(->> coll (map f) …)` | **The collection comes first**, in every collection function: `(map xs f)`, `(fold xs init f)`, `(reduce xs init f)`, `(seq/filter xs pred)`. With one argument convention there is one pipe, `->`, and `$` places the threaded value anywhere else: `(-> n (- 100 $))`. There is no `->>` (ADR-308): Brood's `map` takes exactly one collection, so nothing forces a collection-last order. A callback's own parameters keep the familiar order, `(fn (acc x) …)`. | `(map inc xs)`: `nest check` reports the argument types, and at run time `seq: expected a collection, got :fn`. `->>` is unbound, with a hint naming `->` and `$`. |
| `(condp = x 1 :a 2 :b)` | **The subject comes first**, as in `case` and `match`: `(condp x = 1 :a 2 :b)`. Each test runs as `(pred x test)`, value first like every data-first predicate: `(condp line string/starts-with? "#" :heading "-" :item :text)`. | An error that names the swap: `condp: the second argument, x (2), is not a function — the subject comes first: write (condp x = …)`. A literal in that slot is refused when the form expands. |
| `(cond … :else x)` | The catch-all is the bare symbol **`else`**. `cond` refuses any constant test when it expands (ADR-518): a truthy one always runs, and a falsy one never can, so a constant there is always a mistake. A keyword is a constant. | A compile error that names `else`. |
| `(or (parse s) default)` for a parse that can fail | A function that cannot interpret its input returns a **failure** value (`(string/->number "x")`, ADR-310), and a failure is **truthy**: `nil` means absence, a failure means a value could not be produced, and the two are kept apart. Say which one you mean: `(if (failure? n) 0 n)`, or narrow by type, `(if (int? n) n 0)`. See [Truthiness](#truthiness). | `nest check` warns that the failure tests true. Unchecked, the failure itself flows on as the value. |
| `let` on an `*earmuffed*` var (Common Lisp, Emacs Lisp); bindings that follow work into a `future` | **`let` is always lexical**, even for a `defdyn` name; only `binding` rebinds dynamically. Dynamic bindings are **per-process**: a `spawn`ed child starts from the defaults, the way data is never shared between processes. To give a child a value, pass or send it. See [Dynamic variables](#dynamic-variables). | Silent: the callee or the child reads the default. |
| `({:a 1} :a)`, `([10 20] 0)` | A **keyword** is callable, as an accessor: `(:a m)`, and `(map people :name)` (ADR-165). Nothing else data-like is: use `(get m :a)` and `(nth v 0)`. A callable map would be a second spelling of `get`, and a callable vector or set would answer by index or by membership, an ambiguity Brood refuses. | A clean error whose hint names `get` / `nth`. |
| `()` as an empty list that is truthy and not `nil` | **`()` is `nil`**: `(= () nil)` is `true`, and the empty list is the one empty collection that is falsy (`[]`, `{}`, `#{}`, `""` are truthy). Test emptiness with `(empty? x)`, never a bare `(if x …)`. See [Truthiness](#truthiness). | Silent: an empty list result takes the else branch. |

Within a *single* clause, optional and rest arguments use the Common-Lisp /
Emacs-Lisp spelling (`&optional`, `&`), described under
[Parameter lists](#parameter-lists). Brood *also* has clauses by argument count — but
the param lists are **lists** `(x y)`, not vectors `[x y]`, and clauses are tried in order
(see [`fn`/`defn` clauses](#fn--defn-clauses)). The list-not-vector spelling
is the one piece that can't be guessed from Clojure; it has to be read.

## Coming from Common Lisp or Scheme

Brood is a Lisp-1 with immutable data, so the habits that differ most are the ones about
names and change. [`brood-among-lisps.md`](brood-among-lisps.md) places it among the other
Lisps and lists, with the decision behind each, what a Lisper will look for and not find.

| Common Lisp / Scheme habit | Brood |
|---|---|
| `(defun f (x) …)`, `(define (f x) …)`, `(lambda (x) …)` | `(defn f (x) …)` and `(fn (x) …)`. |
| `(let ((a 1) (b 2)) …)` | Bindings are **flat**: `(let (a 1 b 2) …)`, each seeing the ones before it. |
| `(funcall f x)`, `#'f`, `(apply #'f args)` | One namespace (a Lisp-1): `(f x)`, `f`, `(apply f args)`. |
| `car`, `cdr`, `mapcar`, `(reduce f xs)` | `first`, `rest`, and **the collection first**: `(map xs f)`, `(fold xs init f)`. |
| `setf`, `setq`, `set!`, `incf`, a mutable vector or hash table | Nothing changes once made. Return a new value; `def` rebinds a global; a `Table` or a process holds state that must change ([Immutability](#immutability)). |
| `loop`, `do`, `dolist`, `dotimes`, named `let` | `doseq` for effect (`(doseq (i (range n)) …)`), `for` to collect, `fold`, or tail recursion, which never grows the stack. |
| `progn`, `begin` | `do`. |
| `unless` | `(when (not test) …)`. |
| `eq`, `eql`, `equal`, `eqv?`, `equal?` | One `=`, structural and type-sensitive: `(= [1 2] [1 2])` is true, `(= 1 1.0)` is false. |
| `t`, `#t`, `#f` | `true` and `false`. Only `nil` and `false` are falsy; `()` is `nil`, so the empty list is falsy as in Common Lisp. |
| `` `(a ,b ,@c) `` | `` `(a ~b ~@c) ``. |
| `&rest`, `&key` | `&` for the rest; a trailing options map instead of keywords (ADR-163). `&optional` is spelled the same. |
| `handler-case`, `with-handlers`, `guard`, restarts | `(try … (catch pattern …))`, and a function that cannot do its job usually returns a **failure value** rather than raising ([Errors](#errors)). |
| `call/cc`, `dynamic-wind` | Processes and supervision. |
| `#\a` | No character type: `"a"`. |
| Symbols read upcased (`'Foo` is `FOO`) | Case-sensitive: `'Foo` and `'foo` are different symbols. |

## Data types

| Type | Examples | Notes |
|---|---|---|
| Nil | `nil` | The empty value; also the empty list. |
| Boolean | `true`, `false` | |
| Integer | `0`, `42`, `-7`, `0xFF`, `0b1010`, `0o17` | 64-bit; arithmetic is overflow-checked. A radix prefix (`0x` hex, `0b` binary, `0o` octal, either case, sign in front) is spelling only — `0xFF` *is* `255`, `:int`, and prints as decimal. A result out of `i64` range promotes to an arbitrary-precision **bignum** rather than wrapping, and demotes back when it fits again — so the integer type is unbounded in practice. |
| Float | `3.14`, `-0.5`, `1e3`, `inf`, `nan` | 64-bit. **`inf`, `-inf` and `nan` are reader literals** — those three bare tokens are floats, not symbols, so they can't be used as names (the digit-required rule below has these three exceptions). Test them with `math/infinite?` / `math/nan?`; `=` reports NaN as equal to nothing, per IEEE. |
| Decimal | `1.50M`, `0M`, `-3.14M` | Exact arbitrary-precision base-10, for money and Postgres `numeric` — values a float can't hold (`(+ 0.1M 0.2M)` *is* `0.3M`). The literal is a trailing `M`; `(decimal/of x)` builds one from a string, int, bignum or float. Scale is significant in arithmetic (see [Arithmetic](#arithmetic)) but **not** in `=`, which compares values (`1.5M` = `1.50M`). |
| Ratio | `1/2`, `-3/4`, `22/7` | Exact rational (`num_rational::BigRational`), always **reduced** with a positive denominator (ADR-196). `1/2` is a literal; **`/` on integers is exact** — `(/ 1 2)` is `1/2`, `(/ 6 3)` is `2` (a denominator of 1 demotes to an integer, so `4/2` IS `2`). Does the full arithmetic tower: ratio+int/ratio → ratio, ratio+decimal → ratio (lossless), ratio+float → float (contagion). `->float`/`decimal/of` convert out; `math/numerator`/`math/denominator` read the parts. `=` is by value (`1/2` = `2/4`). |
| String | `"hello\n"` | Escapes: `\n \t \r \e \0 \\ \"` (`\e` is ESC, for ANSI terminal control), `\xHH` (two-hex-digit byte), `\u{H..H}` (1–6-hex-digit Unicode codepoint). A malformed `\x`/`\u{}` is a read error, and so is an unknown **alphabetic** escape (`\d`, `\w`, `\s`, …) — that's almost always a regex class written in a plain string, where dropping the backslash would silently break the pattern, so write `\\d`. A `\X` escape of punctuation or a digit (`\.`, `\/`, `\1`) is literal `X` (how you write a literal metacharacter in a regex string). Readable printing is the inverse: it re-escapes `\n \t \r \e \0 \\ \"` by name and any other control char as `\u{H..H}`, so a printed string always re-reads to the same value. |
| Symbol | `foo`, `+`, `my-fn`, `empty?`, `++`, `...` | Names; interned. **A token that leads with a digit — or a sign/dot immediately followed by a digit — must be a number** (ADR-169): if it isn't one Brood has (`0x1F`, `1_000`, `1N`, `1+`, `12-34`; but `1/2` *is* a ratio now — ADR-196) it's a *reader error*, never a symbol, so those tokens stay reserved for future numeric syntax. A sign or dot with **no** digit behind it is not digit-led and stays a symbol — `+`, `-`, `...`, `.foo`, `foo.`, `--5`, `++`. A symbol whose name isn't a clean token — one built via `(symbol "a b")` with whitespace, delimiters, an empty name, or a spelling that would read as a number/keyword (including a reserved one, `(symbol "1+")`) — prints (readably) and reads back as a `\|…\|` **bar-quoted** symbol (`\|a b\|`, `\|1+\|`, `\|\|` for empty; `\|`/`\\` escape a literal bar/backslash), so every symbol round-trips through `pr-str`/`read`. |
| Keyword | `:ok`, `:else`, `:\|a b\|` | Self-evaluating named constants. Like symbols, a keyword whose name isn't a clean token (e.g. `(keyword "a b")`, `(keyword "")`) prints and reads as `:\|…\|`. |
| List | `(1 2 3)`, `()`, `(1 . 2)` | Cons cells; `()` is `nil`. Quote to keep as data: `'(1 2 3)`. A dotted tail `(a . b)` makes an improper list (round-trips with the printer). |
| Vector | `[1 2 3]` | A data type with O(1) indexing. Evaluates its elements. |
| Map | `{:a 1 :b 2}`, `{}` | Immutable key→value associations. Iteration order is hash-derived, **not** insertion order (see [Maps](#maps)). Evaluates its keys and values. Any value can be a key (compared structurally). |
| Set | `#{1 2 3}`, `#{}` | Immutable collection of distinct elements; its own kind, never `=` to a map (see [Sets](#sets)). |
| Bytes | `#b"abc"` | Immutable byte sequence, the binary counterpart of a string (see [Bytes](#bytes)). |
| Function | `#<fn name>`, `#<native cons>` | Closures and builtins. |
| Record | `#point{:x 3, :y 4}` | A value of its own kind holding named fields and a nominal identity, built by a `defrecord` constructor (ADR-542; see [Records](#records)). `type-of` is `:record`. |
| Ref | `#<ref 0>` | A unique, opaque reference token from `(ref)` — no literal syntax; the only way to make one. Used to tag a request to its reply (see [Processes](#processes-concurrency)). |
| Pid | `#<pid a/7>` | A process id from `self`/`spawn`; carries node identity (`node/id`). No literal syntax. The location-transparent handle for `send` — local or across a node link (see [Distributed nodes](#distributed-nodes)). |
| Failure | `#failure{:message "…"}` | What a function returns when it cannot interpret its input — `(string/->number "abc")` (ADR-310). No literal syntax; build one with `failure` (or `failure/with`, to carry fields beside the message), test with `failure?`, read it with `error-message` and `fields`. **Truthy**, so it is never silently taken for absence (see [Truthiness](#truthiness)), and distinct from `nil`, which means *absence*. Prints self-identifyingly and is deliberately not re-readable — reconstructing one from text would manufacture a provenance it never had. See [Errors](#errors). |

### Truthiness

Only `nil` and `false` are falsy. **Everything else is truthy**, including `0`, `""`,
empty collections like `[]`, `{}`, `#{}` — **and a `failure`**.

A failure being truthy is deliberate. It was falsy for one release, so that
`(or (string/->number s) 0)` would keep working unchanged; that turned out to be the
problem rather than the feature. The call sites needing no edit were the ones *silently
swallowing* the failure — `(or (string/->number p) 0)` read `"x"` as `0` and said
nothing. Falsiness also cannot separate "it failed" from "there is nothing here", which
is the very collapse this design exists to undo: `nil` means absence, a failure means it
could not be produced. To treat a failure as absent you now say so, with `failure?`.
See [Errors](#errors).

> **The one asymmetry — an empty *list* is falsy.** The rule is purely
> `nil`/`false`, but the **empty list is `nil`** (`()` ≡ `nil`), so a list is the
> one collection whose empty value is falsy, while an empty vector/map/set/string
> is truthy:
>
> ```clojure
> (if [] :yes :no)   ;=> :yes   ; empty vector — truthy
> (if "" :yes :no)   ;=> :yes   ; empty string — truthy
> (if {} :yes :no)   ;=> :yes   ; empty map — truthy
> (if () :yes :no)   ;=> :no    ; empty list — () is nil, so FALSY
> ```
>
> This bites when a function may return either a list or `nil` and you branch on
> the result directly: an empty-list result takes the `else` branch. **To test
> for emptiness uniformly across every collection, use `(empty? x)`** — never a
> bare `(if x …)`.
>
> It is kept on purpose (ADR-620): lists are mostly code here, and on a list alone
> `(if xs (f (first xs) (rest xs)) done)` is the recursion every classic Lisp writes —
> the checker narrows `xs` to a non-empty list in the `then` branch. Where the
> asymmetry bites is a value that may be a list **or** another collection, and
> `nest check` flags exactly that: a truthiness test (`if`, `when`, `and`, `or`, `cond`)
> whose type admits a possibly-empty list and a vector, map, set or string. Silence it
> on purpose with `(check-allow :list-truthiness …)`.

## Immutability

**Brood is an immutable language.** Once a value exists, nothing changes it; once
a binding is made, it never changes. Concretely:

- **Data is immutable.** There are **no data-mutation primitives** — no
  `set-car!`, `vector-set!`, `string-set!`, no atoms, refs, or cells. Operations
  like `cons`, `assoc`, `conj`, and `append` return a **fresh** value and leave
  their inputs untouched. Strings, lists, and vectors are read-only after
  construction.
  A `conj` (or `into`) onto a vector costs O(1) amortised, not a copy of the vector: the
  newest vector over a buffer appends into its spare room, past the length any existing
  vector reads, and every other append copies (ADR-511). No value changes either way, so
  a `conj` loop onto a vector is linear and every older version stays exactly as it was.
- **Local bindings never change.** A `let` or `fn` binding is fixed for the life
  of its frame — there is no `set!` to rebind it.
- **The one mutation is `def`.** `def` rebinds a name in the **global**
  environment (even when written inside a function). This is *binding* mutation,
  not data mutation, and it exists for one reason: **live redefinition / hot
  reload** — the project's north star (ADR-013). A running process sees a `def`'d
  change on its next global lookup.
- **…but not of the language's own functions.** A name that shipped inside the
  `brood` binary — a prelude function or macro, a Rust builtin, or a function from an
  embedded std module — is **reserved**, and `(def get …)` is an error (ADR-166). The
  boundary is the binary: *if it came with Brood it is reserved; if you or a package
  author wrote it, it is yours*. Hot reload is untouched, because it was always about
  your code. The language's own functions are fixed for the life of the binary, and
  that is what lets the compiler bind those names early. Three things stay
  legal: a local `let` shadow (that binds a local, it isn't a redefinition), a
  module-scoped `(defn get …)` (which defines `your/mod/get`), and rebinding the
  prelude's *data* registries such as `*load-path*` (prelude functions do that
  themselves — the rule reserves shipped **functions**, not every shipped name). A
  **dynamic variable is never reserved**, whatever it holds: `defdyn` declares a name
  rebindable, so `(def *out* my-port)` — a permanent output redirect — still works
  alongside the scoped `binding` form.

  **Where redefinition stops**, in one place:

  | You write | Result |
  |---|---|
  | `(def my-fn …)`, `(def my/mod/f …)` — your own globals, your packages | redefines; running code sees it on its next lookup (hot reload) |
  | `(def get …)`, `(defmacro when …)` — the prelude, a builtin | refused: reserved (ADR-166) |
  | `(def string/join …)` — an embedded std module's function | refused, whether or not `string` has loaded yet: the `def` loads it first |
  | `(def if …)` — one of the eight special forms | refused: `(if …)` is always the form, so the global could never be called |
  | `(def *load-path* …)`, a `defdyn` name | redefines: shipped *data* and dynamic variables are not reserved |
  | `(defn get …)` inside `(defmodule app/x …)` | defines `app/x/get`, a new name of yours |
  | `(let (get …) …)`, a `fn` parameter named `get` | a local; nothing global changes |
  | redefine a `defmethod` / `impl` | ordinary hot reload into the dispatch registry |
  | redefine your own **macro** | later expansions use it; code already expanded keeps the old expansion until it is re-evaluated |
  | `(reflect/undef 'mine)` / `(reflect/undef 'map)` | removes yours / refused for a shipped name |
- **No imperative loop.** There is no `while` (and nothing to make it progress
  without mutation). Iteration is **recursion** — proper tail calls give O(1)
  stack — or, for state that must evolve over time, a **process** (`spawn` /
  `receive`) that carries the state through its own loop.
- **Mutable state, when truly needed, is never a mutable `Value`.** It takes one
  of two shapes: a **process** holding evolving state in its receive-loop, or a
  **Rust-backed opaque resource handle** exposed through
  primitives (the rope/buffer, or the [`table`](#in-memory-tables) store
  below — like a file handle) — mutation hidden behind the kernel, never aliasable
  Lisp data.

**Why it pays off.** Immutability removes the entire shared-mutable-aliasing bug
class and reinforces every other pillar of the system: the tracing GC needs no
write barriers or mutable roots; per-process heaps are trivially `Send` with
copy-on-send messages and no aliasing hazards; the shared `RUNTIME` code region
can be append-only; and it keeps the safe-Rust guardrail (ADR-001) honest. It
also shrinks the core — two fewer special forms (`set!`, `while`). See
[ADR-026](decisions.md) for the full rationale and trade-offs (e.g. repeated
immutable `assoc`/`append` is O(n²); `reduce`/`fold` and future persistent
structures are the mitigations).

## In-memory tables

A `table` is shared, concurrently-mutable key→value state (ADR-107). It's the
escape hatch for the case immutable maps and message-passing don't cover well: state that **many processes read and write
directly**, without routing every access through one owning process's mailbox. It is
a Rust-backed opaque handle (genuine mutable state, never a mutable `Value`):

```clojure
(def t (table/new))               ; create — returns an opaque handle
(table/put t :hits 0)             ; store (overwrites); returns t
(table/get t :hits)               ;=> 0   ; a fresh copy
(table/get t :missing :default)   ;=> :default
(table/contains? t :hits)         ;=> true
(table/incr t :hits)              ;=> 1   ; atomic; default delta 1
(table/incr t :hits 10)           ;=> 11
(count t)                         ;=> 1   ; a table is a sized collection
(table/snapshot t)                ;=> {:hits 11}  ; an immutable map
(table/delete t :hits)
(table/drop t)                    ; free it now (else it goes when this process exits)
```

How it behaves, and why it's safe:

- **Shared by identity, like a pid.** The handle can be `send`'d to (or captured by)
  other processes; every copy names the **same** store. `=` and `table?` compare by
  identity — two tables with equal contents are not `=`.
- **Copy-in / copy-out.** `table/put` stores a **deep clone** of the key and value;
  `table/get` returns a **fresh copy** into the caller. So no two processes ever alias
  a stored value, and the store is invisible to the garbage
  collector — it can't corrupt across a collection.
- **Keys are structural**, identical to map keys (`[1 2]`, `:k`, `"s"`, `42` all
  work).
- **`table/incr` is atomic** — a lock-held read-modify-write, so concurrent counters
  never lose an update. (There is deliberately no closure-based `update`: running
  arbitrary code under the lock can't be made safely atomic. For other atomic needs,
  serialize through a process.)
- **`table/snapshot`** is a consistent point-in-time map — unaffected by later
  mutation — and your enumeration primitive: use ordinary map ops (`keys`, `vals`,
  `reduce`) on it.
- **Local to the runtime.** A table can't cross a node link (send its
  `table/snapshot` — a plain map — instead).
- **It dies with the process that created it** (table ownership, ADR-418), or at
  `table/drop`. Other processes holding the handle then get `no such table`. The
  exception is a table reachable from a **global**: `(def cache (table/new))`, a table
  a `def`'d closure captures, or one inside a `def`'d map lives as long as the runtime,
  whichever process ran the `def` — so a module-level table survives the process that
  loaded the module.

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
with `[1 2]`). Duplicate keys keep the **last** value.

**Iteration order is unspecified — do not rely on it.** `keys`, `vals`, printing,
and seqing a map yield entries in the **CHAMP trie's hash-derived order**
(ADR-040), which is *neither* insertion order *nor* sorted, and may differ across
runtimes or versions. (It is a function of the keys' hashes, not of how you built
the map — so two `=` maps iterate alike — but treat the specific order as an
implementation detail.) When you need a defined order, **sort the keys**
(`(sort (keys m))`) or compare via `seq/frequencies`. Map equality (`=`) is itself
**order-independent**: `{:a 1 :b 2}` equals `{:b 2 :a 1}`.

Maps are immutable — every operation returns a **fresh** map:

| Form | Meaning |
|---|---|
| `(get m k)` / `(get m k default)` | the value at `k`; `nil` (or `default`) if absent. A **wrong-typed key** (a keyword into a vector/list/string) or a non-collection is a *type error*, not the default — `default` means "absent", never "malformed" |
| `(assoc m k1 v1 k2 v2 …)` | a new map with the pairs added/updated (also works on a **vector** with integer indices — replaces, never appends) |
| `(dissoc m k1 k2 …)` | a new map with those keys removed |
| `(contains? m k)` | whether `k` is present (distinguishes a stored `nil` from absence) |
| `(keys m)` / `(vals m)` | the keys / values, as a list, in the map's (hash-derived, unspecified) iteration order — sort if you need a defined order |
| `(map/reduce-kv m init f)` | fold over the entries: `(f acc k v)` left to right → the final acc — the `map` module, like `map/merge-with` below |
| `(merge m1 m2 …)` | combine maps left to right; rightmost key wins (`nil` maps skipped) |
| `(map/merge-with maps f)` | like `merge` over the collection `maps`, but a shared key's value is `(f old new)` — the `map` module (ADR-227; `(:use map)` or qualify); the function comes last (ADR-507) |
| `(update m k f args…)` | a new map with `k`'s value replaced by `(f current args…)` (`current` is `nil` if absent; also works on a **vector** by integer index, which must be in range) |
| `(map/update-vals m f)` / `(map/update-keys m f)` | a new map with `f` applied to every value / key — the `map` module |
| `(map/select-keys m ks)` | a new map of just the entries whose key is in `ks` — the `map` module |
| `(map/zipmap ks vs)` | a map pairing `ks` with `vs` positionally (stops at the shorter) |
| `(map/get-in m path)` / `(map/get-in m path default)` | the value at a nested key `path`, or `default`/`nil`; each step looks up like `get`, so a vector (or list/string) along the path is indexed by position — `(map/get-in {:a [1 2]} [:a 1])` is `2` |
| `(map/assoc-in m path v)` | a nested copy with `v` stored at `path` (intermediate maps created); a vector along the path with an integer key is updated at that index (in range, as for `assoc`) — `(map/assoc-in [[1]] [0 0] 9)` is `[[9]]` — and any other non-map node becomes a fresh map |
| `(map/dissoc-in m path)` | a nested copy with `path` removed (a missing path is a no-op; empty branches are left in place) |
| `(map/update-in m path f args…)` | a nested copy with `path`'s value replaced by `(f current args…)` |
| `(count m)` / `(empty? m)` | number of entries / whether there are none |
| `(map? x)` | whether `x` is a map |

```lisp
(def person {:name "Ada" :born 1815})
(get person :name)                  ;=> "Ada"
(get person :died "unknown")        ;=> "unknown"
(assoc person :field "computing")   ;=> {:name "Ada" :born 1815 :field "computing"}
person                              ;=> {:name "Ada" :born 1815}   ; unchanged
(-> person (assoc :born 1816) (get :born))   ;=> 1816
```

### Keyword accessors

A **keyword is callable** — the one exception to "the head of a form is a function":

```clojure fragment
(:name person)              ; ≡ (get person :name)
(:name person "unknown")    ; ≡ (get person :name "unknown")
(map people :name)          ; the reason for the exception
(seq/sort-by procs :id)
(seq/filter zones :cursor)
```

The point is the last three: a keyword is a first-class *value*, so any higher-order
op can take it, and an accessor no longer needs a throwaway binder
(`(fn (p) (get p :name))`). It works anywhere a function goes — `apply`, `comp`, a
`let`-bound name.

Receivers mirror `get`: a **map** looks up by key, a **set** by membership (yielding
the element), and `nil` is empty. Anything else is a type error **naming the
keyword** — including an integer-indexed collection, because `(:name deps)` where
`deps` is a *list* of maps is the most likely way to get this wrong:

```clojure
(:name {:name "ada"})    ;=> "ada"
(:name #{:name})         ;=> :name        ; membership, like get
(:name nil)              ;=> nil
(:name [{:name "ada"}])  ;=> RAISED :name: expected a map, record, set or nil to look up in, got vector ([{:name "ada"}])
```

The advisory checker knows the form (ADR-167): it flags a receiver that provably
can't be keyed, a wrong arity, and flows a **typed record field** through the result —
so `(string/upper (:x p))` is caught for a `(defrecord pt ((x int) …))` exactly as
`(string/upper (get p :x))` is.

**Only keywords.** A map, vector or set in head position is still an error with a
hint — `({:a 1} :a)` would be a second spelling of `get`, and a callable vector or set
answers by index-or-membership, the ambiguity `contains?` deliberately refuses
(ADR-156/165). Use `(get m k)` when the key is *computed*; `(:k m)` can only ever
mean the literal keyword `:k`.

A map is **seqable as its `[k v]` pairs**: `seq`, `first`, `rest`, `last`, `map`,
`seq/filter`, `fold`/`reduce`, `into`, and `vec` all read it that way, so
`(map m first)` is its keys and `(first m)` is a `[k v]` vector (`nil` for an empty
map). Use `map/reduce-kv` when you want the key and value as separate arguments.

These are thin Brood wrappers (`std/prelude/map.blsp`) over a small kernel of `%map-*`
primitives; the representation is a **CHAMP hash-array-mapped trie** (ADR-040),
which is why `count` is O(1) and structural key equality is O(log n).

### Records

`defrecord` defines a **record**: a value of its own kind (ADR-542) — `type-of` is
`:record`, `map?` is false — holding named fields and a nominal **identity** (a
`:module/name` keyword) that `ability` dispatch reads. The identity lives in the value, not
among the fields, so no map operation can see, write or forge it. `(defrecord point (x y))`
defines a positional constructor and one accessor per field:

```lisp
(defrecord point (x y))
(def p (point 3 4))      ; #<ns>/point{:x 3, :y 4}
(point-x p)              ;=> 3             ; accessor, one per field
(= (assoc p :x 9) (point 9 4))   ;=> true  ; a fresh record of the same type
(fields p)               ;=> {:x 3 :y 4}   ; the fields as a plain map
(record? p)              ;=> true
(record-id p)            ;~> :<ns>/point
(= (point 1 2) {:x 1 :y 2})   ;=> false    ; a record is never a map
(= (point 1 2) (point 1 2))   ;=> true     ; same type, same fields
```

Field access works as on a map: `get`, a keyword call, `assoc`, `update`, `dissoc`, `keys`,
`vals`, `count`, `seq` and `{:keys …}` destructuring all read or change the **fields**, and
`assoc`/`update`/`dissoc` return the same record type. There is no reserved key —
`(assoc {:x 1 :y 2} :__id__ :point)` is just a map with an ordinary key, never a record. A
record prints as `#point{:x 3, :y 4}` (`#geo/point{…}` for a module's record).

The accessors are the reason to reach for it: `(point-x p)` names the field, and a
typo `(point-witdh p)` is a call to an **undefined function** — caught by `nest check`
and at runtime — whereas `(get p :witdh)` silently returns `nil`.

**Equality is nominal and structural.** Two records are `=` when they are the same record
type with equal fields; a record is never `=` to a map, nor to a record of another type with
the same fields. `record?`/`record-id`/`fields` are the identity API. Functional update is
plain `assoc`/`merge`, and the record keeps its type.

A field may carry a type — `(defrecord point ((x int) (y int)))` — and when every
field is typed, `(sig …)` declarations are emitted for the constructor and
accessors, so the advisory checker (and `BROOD_CONTRACTS=1` runtime contracts) see
the field types. See [ADR-130](decisions.md) and `docs/types.md` for the record
type `(record :k T …)` this lowers to.

### Polymorphism: abilities

`defrecord` names a value's *shape*; an **ability** names an operation that different
types implement differently. Abilities are **core** (in the prelude, ADR-168/172):
open generic functions where each op dispatches on the **identity of its first
argument**, and an implementation can be added for any identity — including a built-in
kind — from any module, at any time, without editing the dispatcher.

`defability`/`impl`/`defrecord` are built in — always available, no import, no
`(:use ability)`.

> **Which one: an ability or `defmulti`?** Brood has two dispatch mechanisms, each for one
> job (ADR-523). Use an **ability** — almost always — when the implementation depends on the
> first argument's type: `display`, `seq`, a storage port. Use **`defmulti`** only when it
> depends on more than the first argument, the case first-argument dispatch cannot express:
> a symmetric binary operator, where `(+ money 5)` and `(+ 5 money)` must reach the same
> method ([below](#polymorphism-multiple-dispatch-defmulti), ADR-179). `nest check` reports a
> `defmulti` whose methods vary only the first argument — it is an ability written the long
> way, without the ability's provided ops, `:requires` or checker support.

An argument's **dispatch identity** is one of two things:

- for a built-in value, its `type-of` **kind** — `:int` `:float` `:string`
  `:keyword` `:map` `:vector` `:set` `:nil` `:pid` …;
- for a value built by **`defrecord`**, its **nominal id** — a `:module/name`
  keyword baked in at definition, so two record shapes defined in one module
  dispatch apart.

```clojure
(defmodule geometry)

(defrecord circle (r))
(defrecord rect (w h))

(defability Shape
  "The area of a shape."
  (area (self) :-> float))

(impl Shape geometry/circle (area (c) (* 3.0 (get c :r) (get c :r))))
(impl Shape geometry/rect   (area (r) (* 1.0 (get r :w) (get r :h))))

(area (circle 2))         ;=> 12.0
(area (rect 3 4))         ;=> 12.0
```

Built-in kinds work the same way, with **`:default`** as the fallback:

```clojure
(defability Size (size (self) :-> int))
(impl Size :int      (size (n) n))
(impl Size :string   (size (s) (count s)))
(impl Size :default  (size (_) -1))

(size 7)          ;=> 7
(size "hello")    ;=> 5
(size 1.5)        ;=> -1
```

**Retracting an impl.** `%unimpl` is the inverse — the named ops, or every op the
ability declares when none are named:

```clojure
(defability Size (size (self) :-> int))
(impl Size :default (size (_) -1))
(impl Size :int (size (n) n))
(size 7)                    ;=> 7
(%unimpl Size :int)         ;=> 1   ; impls removed
(size 7)                    ;=> -1  ; falls back to :default; no :default → a no-impl error
```

The slot becomes **unclaimed**, not `:default`-y: a later `impl` of any tier takes it,
because precedence compares against an incumbent and there is none. It returns how many
impls it removed, so a retraction is distinguishable from a no-op, and it is idempotent —
retracting what was never there is a quiet zero. `%unregister-impl` is the function
underneath, for a caller holding names as values.

Two things `%unimpl` will not do: retract the language's own `:default` for an ability the
runtime dispatches on itself (`Display`, `Inspect`, `Seqable`, `Conjable` — that one line
would leave every value unprintable, so it errors and points at `impl` for your own id), and
touch any impl but the one named (retracting one id leaves its siblings, including `:default`,
in place).

Registration was open and reversible in every way but this one, and a registry with no
inverse cannot be returned to a known state: a test that must not leak into the next, a
REPL undoing an experiment, a hot reload retracting an impl the new source no longer
declares (ADR-204).

**Op arity.** An op dispatches on its **first** argument, so it must take at least
one — a zero-arg op `(op ())` is a clean expansion-time error. Beyond the first, an
op may take more fixed arguments (`(fetch (self k))`) or a `&`-rest
(`(cat (self & rest))`, dispatched on `self`, the rest passed through). An `impl`
must match the declared op's arity: a fixed impl exactly, a variadic impl as long
as it accepts that arity — `nest check` and load-time registration both flag a
mismatch.

**Provided ops (default bodies).** An op whose spec carries a **body** —
`(op (args) :-> ret? body…)` — is *provided*: `defability` registers that body as the
op's `:default` impl. An `impl` that supplies only the **required** ops (the bodyless
specs) inherits every provided op; an id-keyed impl of a provided op **overrides** its
default. Write the minimal contract and derive the rest (Rust/Haskell *provided methods*) —
implement `priority`, get the rest free:

```clojure
(defrecord task (title level))

(defability Priority
  (priority      (self) :-> int)                                              ; required
  (more-urgent?  (self other) :-> bool (> (priority self) (priority other)))   ; provided
  (same-urgency? (self other) :-> bool (= (priority self) (priority other))))  ; provided

(impl Priority task (priority (t) (task-level t)))   ; only the required op
(more-urgent? (task "deploy" 3) (task "lunch" 1))    ;=> true   ; via the provided default
```

(Ordering itself is not an ability: `compare-to` is a `defmulti` over the operand pair —
see [Multiple dispatch](#polymorphism-multiple-dispatch-defmulti).)

To **override** a provided op for a type, add its method to that type's `impl` (the same
form as the required ops), since each `impl` form is checked for completeness on its own.
A provided op called on an id that implements *none* of the ability runs the default body,
which raises the ordinary `no-impl` for the required op it delegates to.

**Deriving (`:derives`).** A record can auto-generate an ability impl instead of writing it,
Rust `#[derive]` style. The ability declares **how** it derives itself
with a `:derive-record` recipe (field names → `impl` method forms for its *required* ops);
the record opts in with `:derives`:

```clojure
(defability Columns
  (columns (self) :-> vector)                       ; required — derived
  (ncols (self) :-> int (count (columns self)))     ; provided — composes with derive
  :derive-record
  (fn (flds)
    (list `(columns (r) [~@(map flds (fn (f) `(get r ~(keyword (->string f)))))]))))

(defrecord point (x y) :derives [Columns])
(columns (point 3 4))    ;=> [3 4]     ; synthesized
(ncols   (point 3 4))    ;=> 2         ; provided op, on the derived required op
```

The recipe runs at **load** (via `derive-into`), so the ability's `defability` need only be
loaded before the record is used. Deriving an ability that declares no `:derive-record` is a
clean error; the checker treats a derived record as implementing every op of the ability, so
it satisfies call-site and `:sealed` checks.

**Single dispatch — and its multiple-dispatch sibling.** An *ability* op dispatches on the
**first argument only** (Rust-trait / Clojure-protocol style). For a unary op (`->string`,
`->seq`) that is all there is to it. An op that *combines two values* — arithmetic and
ordering — needs to see **both** operand types, which single dispatch cannot express
symmetrically (`(+ money 5)` could dispatch on `money`, but `(+ 5 money)` cannot). So those
live in the **`defmulti` multiple-dispatch seam**, not in an ability: `Num`
(`num/add`/`num/sub`/`num/mul`/`num/div`) and `Ord` (`compare-to`) are **multimethods**
dispatching on the operand *pair* (see [Multiple dispatch](#polymorphism-multiple-dispatch-defmulti)
below, [ADR-179](decisions.md)). Author `(defmethod num/add [money money] …)` and
`(defmethod num/mul [money :int] …)`; `+`/`*` are commutative, so the mirror is derived and
`(+ 5 (money 50))` and `(+ (money 50) 5)` agree. There is **no implicit coercion** — a pair
with no method is a loud `no-method`, never a silent conversion.

**Op names are unique per module.** Each op becomes a generic function bound in the
declaring module, so two abilities in *one* module declaring the same op name (`size`)
would clobber each other's generic function. That collision is warned at load and is a
`nest check` reject (ship-blocking, advisory in the live image) — rename one op. A
*different* module declaring the same op name is fine: it binds a distinct `<module>/op`
global. (A use-site clash — `(:use)`ing two modules that each export `size`, then calling
bare `size` — is the ordinary module-import ambiguity the module system resolves, not an
ability-specific rule.)

> **The `impl` dispatch id resolves to the way `%identity-of` produces it.** `impl`
> and `:sealed` share one helper (`%ability-id-kw`): a **bare** record symbol is
> qualified against the current namespace (`circle` → `:<ns>/circle`), an
> already-`/`-qualified symbol is used as written (`geometry/circle` — the form to
> use for a record from *another* module), and a keyword id (`:int`, `:default`) is
> left untouched. So a same-module `(impl Shape circle …)` and `(impl Shape
> geometry/circle …)` register under the same id a value presents; the earlier
> bare/qualified asymmetry is fixed ([KI-15](known-issues.md)).

A missing implementation is a **loud, named error** — `ability Shape/area: no impl
for :geometry/circle — have (…)`, listing the ids that *are* implemented — never a
silent `nil`. The thrown value is a **structured map**, `{:kind :%no-impl :code "E0090"
:message … :ability :op :id :have}`, so a handler can branch on the parts —
`(catch e (get e :id))` — rather than parse the string; `error-message` still
returns that same human text.

**Records dispatch as themselves; plain maps do not.** `(type-of r)` is `:record` for a
`defrecord` value, and its nominal id lives in the value (ADR-542). A `defrecord` value
dispatches on its `:module/name` id; a plain map, **even one carrying a `:type` field**,
stays `:map` and lands on the `:map` impl. That is the ADR-011 line —
the identity is explicit and construction-time, never inferred from a field.

A record whose contents are not its fields (a multimap, a sorted map) implements
`Lookup`, and bare `get` asks it on a **miss**: a key that is not a field, or a field
holding `nil`. That holds with a default too: `(get mm :k [])` is `:k`'s values when the
multimap holds them, and `[]` only when `lookup-get` also answers `nil` (KI-298). A
present field is answered without asking. A plain map never consults `Lookup`, and
`(get {:a nil} :a 99)` stays `nil`.

> **On-ramp: reach for `defrecord` *before* you want polymorphism.** Because dispatch is
> nominal, there is no structural path — you cannot `impl` an ability for "a map whose
> `:kind` is `:circle`". If you are building a value on plain maps told apart by a tag
> field and later want to dispatch on the kind, the move is to make it a `defrecord` first
> (its `record-id` then carries the identity), then `impl` against that. This is a deliberate constraint, not a gap — it keeps identity construction-time
> and explicit — but it lands as a small refactor if you defer it, so prefer records for
> any value you expect to dispatch on.

**A driver is just a value.** Because dispatch is on the first argument, "swap the
backend" needs no config indirection and no module-atom dispatch — you pass a
different value:

```clojure
(defmodule store)

(defability Store (fetch (self k)))
(defrecord pg  (pool))
(defrecord mem (data))
(impl Store store/pg  (fetch (db k) (str "pg:" (get db :pool) "/" k)))
(impl Store store/mem (fetch (db k) (get (get db :data) k)))

(fetch (mem {"a" 1}) "a")          ;=> 1
(fetch (pg "main") "a")            ;=> "pg:main/a"
;; swapping the driver value swaps the impl — no config, no module atoms
```

**Sealed abilities (provided ops excluded).** `:sealed [id …]` names the **closed** set of ids the ability
is meant to cover, and `nest check` then flags any member missing a direct impl of
any **required** op (a `:default` does not count; a *provided* op is satisfied by its
default, so members need not implement it). Runtime dispatch is unchanged —
sealing is a contract, not a restriction:

```clojure
(defability Shape :sealed [circle rect] (area (self) :-> float))
```

**Super-abilities (`:requires`).** `:requires [A B …]` names prerequisite abilities — an id that
implements this ability must also implement each of them, which `nest check` enforces (ADR-193).
It's Rust's `trait Ord: Eq`: implement `Ord` for a type and forget `Eq`, and you're told at check
time, not by a runtime `no-impl` later. Advisory and checker-only, like `:sealed` (late binding
means it can't gate); a required ability's *provided* op is satisfied by its default.

```clojure
(defability Named    (label (self) :-> string))
(defability Priority :requires [Named] (priority (self) :-> int))
(impl Priority task (priority (t) …))   ; warns unless `task` also impls Named
```

**Introspection and helpers.** `(satisfies? 'Shape x)` is true when every declared
op resolves for `x` (directly or via `:default`), so a caller can branch instead of
letting the op raise. `(record? x)` / `(record-id x)` / `(fields x)` are the clean
view of a record — `fields` returns its fields as a plain map.
`(%ability-ops 'Shape)` exposes the declared op specs as data, the same hook the
checker and LSP read.

The id is part of the value, read with `record-id`; the record's **collection view is its
fields**: `seq`/`count`/`keys`/`vals`
— and `map`/`seq/filter`/`fold`/`for`/`into`, which coerce through `seq` — see only the
fields, so `(count (circle 2))` is `1`. This is the `Seqable` ability (op `->seq`,
default = the fields); a custom-collection record overrides it to define its own
iteration — and `first`/`rest`/`empty?`/`nth` read that view too. A record that knows its
size without walking its view implements `Counted` (op `->count`), which `count` and
`empty?` consult first: a queue answers both in O(1). `(fields r)` gives the fields as a
plain map explicitly.
A record being `≠` a bare map with the same fields, and printing with its id, are
*intended* (records are nominal), not leaks; `json/encode` writes the fields, so the id
never reaches the wire.

**What the checker does.** `nest check` verifies each `impl` provides the ability's
declared ops at the right arity and flags an op the ability never declared. It also
warns at a **call site** when an op is applied to an argument of statically-known
identity — a literal, a direct `defrecord` constructor call, or a record-typed
variable — for which no impl and no `:default` is registered. Two abilities that
declare the **same op name** in one namespace would have the second's generic function
shadow the first's; `defability` warns at load when that shadowing is real.

**Typed ops — the `:-> RET` return type (ADR-180).** An op spec's optional `:-> RET`
tail is a real type the checker consumes, not a comment. Two things follow from it:

- **The return flows into inference at every call site.** An op is a generic `defn`
  whose body *is* the dispatch machinery, so its own type is opaque — the declared
  return is the only static handle on what the call yields. `(area shape)` for `(area
  (self) :-> float)` types as `float`, so `(+ (area s) 1.0)` checks and `(string/upper
  (area s))` is flagged. This is the single most useful place to recover type
  information, because it's the polymorphic boundary where it would otherwise be lost.
- **Each `impl` body is checked against it.** An impl of `(size (self) :-> int)` whose
  body provably yields a non-`int` (a string literal, a `bool`) warns. The check is
  **gradual and false-positive-clean** — exactly like a `sig` return: an
  over-approximated body (a call result, an unknown param) is `dynamic` and defers; only
  a body *provably disjoint* from the declared return warns. A `:-> any` op imposes no
  constraint (`any` is the gradual unknown, not a top type). All advisory — the return
  type never gates the live image, and dispatch is unchanged.
- **Parameters may be typed too — `(name T)`.** An op spec's params are bare symbols by
  default; wrap one as `(name T)` to declare its type: `(scale (self (factor float)) :->
  int)`. The type is the same checker-only annotation as `:-> RET` (dispatch is still on the
  first argument's identity), and it does two things: an **argument** at that position is
  checked at every call site — `(scale s "x")` warns "argument 2 expects float" — and the
  **impl body** sees the param at that type, so returning it against a disjoint `:-> RET` is
  caught. Same gradual, false-positive-clean relation as a `sig` param: a precise argument is
  checked `⊆`, a dynamic one `∩ ≠ ⊥`, an unknown one defers. Untyped positions (the common
  case, including `self`) impose nothing.

**A sealed ability *is a type* (ADR-181).** A `:sealed` ability names a closed member set,
so its name is usable directly in the type grammar — it denotes the **union of what its
members denote**. `Shape :sealed [circle rect]` means `(or circle rect)` as a type, so you can
write `(sig total (Shape -> float))` or return one from an op (`(scaled (self) :-> Shape)`).
A member record satisfies it (records are open, so extra fields are fine); a non-record is a
provable mismatch; anything whose type isn't pinned down defers — sound, no false positives.

> **Members may be built-in kinds, not just records.** `impl` dispatches on either, so
> `(defability Numeric :sealed [:int :float :decimal :ratio] …)` is legal, and such a member
> denotes **its own lattice point** — an int, not a record whose id is `:int`. Mixing the
> two in one seal works and keeps both halves exact: `(defability Money :sealed [:int usd] …)`
> types as `int | usd`, and a `eur` is a provable mismatch. (Until ADR-542 a record was a map
> to the checker, so the record half of a mixed seal widened to `map`.) (Until 2026-08-28 every member was turned into a record shape, so a kind-sealed
> ability's domain **rejected its own members** — `(use-it 42)` warned against
> `{__id__: :int | :float}`, which no int is. Only passing a real member exposed it; the
> exhaustiveness gate and non-member rejection both looked right.)

**Doctests.** An indented line in a docstring holding a form followed by ` ;=> ` and its
result — `    (count [1 2 3])   ;=> 3` — is a runnable example: the form is evaluated and must
equal the result, which is *read* as a value (so `;=> (:a :b)` is a list, not a call). The
result is a comment, so the line stays Brood you can paste. `nest test` runs the examples in
the project's own modules after the suite passes, and `nest doctest` runs them on their own —
a wrong example is worse than none, since it reads as authoritative and no other gate sees it
(the checker does not evaluate docstrings). The engine is `std/tool/doctest.blsp`;
`doctest/failures-under` scopes a run to one package's surface, and
`tests/doc_examples_test.blsp` is what points it at the standard library. A line still
written with the retired ` → ` marker is reported, never skipped.

A function whose result depends on the world — a file, the clock, a socket, a pid, the loaded
image — cannot be pinned by a ` ;=> ` example. Write an **illustration** instead, with ` ;~> `:
`    (file/slurp "notes.txt")   ;~> "…the file's text…"`. It is not evaluated,
but doctest still reads its left side and fails it when the form does not read, calls a
function that does not exist, or passes an argument count no arm of that function takes — so a
rename, a typo or an argument reorder is caught (a `…` argument stands for any number); the
example audit counts it.

    "Twice `n`.

        (demo/double 4)   ;=> 8"

An example runs at the **top level**, not inside the module of the name it documents —
so it is written with the full namespace: `(demo/double 4)`, never
`(double 4)`. It then pastes into any REPL as it stands. Only public names are checked, so the
qualified name is always callable; a bare call to the module's own function fails naming the
spelling to write (`examples run at the top level: write `demo/double``).

The same markers work in **markdown**: `nest doctest README.md docs/guide.md` runs every
```` ```clojure ````, ```` ```lisp ```` and ```` ```brood ```` fenced block, each in a process of its
own under a 10 s timeout. A block is a small program: unmarked lines are setup, run in order
(setup that raises fails the block), and each ` ;=> ` / ` ;~> ` line is checked as above — a
marked line may close a form the lines above it opened. A block tagged
```` ```clojure fragment ```` is never run, nor is an untagged block or another language.

Plain `nest doctest` runs the markdown a project's manifest names in `:doc-paths` after its
docstrings — files, directories (every `.md` under them) and `!`-prefixed paths to leave out,
`:doc-paths ["README.md" "guides" "!guides/drafts"]`. Without the key it is `README.md` when
there is one; a path the key names that does not exist is a failure, not a silent skip.

The expected side is **read**, not evaluated, so `(:a :b)` is the list it depicts rather than
a call; and values are compared as values, so a map's key order (the trie's, not insertion
order) cannot break an example. A trailing `; comment` is ignored, and only *indented* lines
count — inline `e.g. …` prose is left alone.

**Integer division has a type: `int | ratio`.** Brood's `/` is **exact** — it yields an `int`
when it divides evenly and a `ratio` when it does not (`(/ 4 2)` → `2`, `(/ 3 2)` → `3/2`), and
never a float, at any arity (`(/ 2)` → `1/2`). So `(sig f (int -> float))` over `(/ x 2)` is a
provable mismatch, as is feeding the result somewhere non-numeric. One float operand still
makes the whole expression a float (contagion wins). Declaring `int` over it is *not* flagged:
for an even numerator that is correct, and proving it would need range analysis (ADR-011).

**A record name is a type.** `(sig area (circle -> float))`, `:-> circle` — a record is the
language's nominal type, so its name resolves in type position to that record type: the
`record` kind narrowed to one id, its fields open. Open, so a record carrying extra fields
(`(assoc (circle 2) :z 3)`) is still one; a *different* record, a plain map or any other
non-record is a provable mismatch. `record?` narrows both ways — to a record in the then
branch, to what is left in the else branch — and a map type is never a record type, however
its fields line up. Both the bare
spelling (inside the declaring module) and the qualified one (`shapes/circle`) work; a bare
name that two modules both claim **declines** rather than guessing, so it reads as an unknown
type rather than the wrong one. A base type name outranks a record that took the same id — in
a type expression `int` is the int kind, even where a root-namespace `(defrecord int …)`
exists.

**Any ability name is a type (ADR-186).** An *open* ability (no `:sealed`) has no finite
member set, so it resolves to the permissive **`any`** — a `sig` mentioning it (`(sig render
(Display -> string))`) still *checks* (the return and other params flow), the open-ability
parameter just accepts anything. That's the sound choice: an open ability's impls are late
and may cover any value, so no argument can be rejected on the type — the "does this value
implement it" safety is enforced at the op *call sites* instead. Sealed abilities keep their
precise finite-union type; both work for abilities declared in *this* file or reachable
through `(:use …)`.

**Ability bounds — this *is* `where T: Ability`.** Because an ability name is a type, a `sig`
parameter typed with a **sealed** ability is exactly a bound: "this argument must satisfy the
ability." `(sig draw (Shape -> string))` requires a `Shape`, and a caller passing a non-member
is flagged — whether or not `draw`'s body uses a `Shape` op. Two abilities intersect the
ordinary way, so **`(and A B)` is a multi-ability bound** (Rust's `T: A + B`):

```clojure
(sig render ((and Shape Comparable) -> string))   ; must be a Shape AND a Comparable
```

An **open** ability as a bound is deliberately permissive (`any`, above) — its impls are late
and may cover any value, so no caller can be soundly rejected; the check happens at the op call
site instead. There is no separate bounds syntax and none is needed: name the sealed ability
(or intersect several), and the checker enforces it — no annotation of the bound beyond the
type itself.

> **Direction: [ADR-172](decisions.md) (amended 2026-07-28).** This open runtime model
> is kept open — `impl` stays legal for any ability and any id (primitive, owned, or
> someone else's) — and made **deterministic and app-sovereign** by a precedence ladder
> alone: `app > type-owner > ability-owner > :default > native`, with same-tier
> cross-module collisions warned (shipped). The amendment **dropped** the earlier
> `impl`-only-what-you-own orphan rule and the `bridge` form: `bridge` expanded to the
> same registration as `impl` (no runtime substance), and the orphan rule guarded a
> multi-third-party-library collision greenfield Brood doesn't have. Dispatch now runs
> through a **polymorphic per-op inline cache** (`%dispatch`, 4-way) that deopts on
> reload (a `def *impls*`/compaction bumps the epoch), and `Display` is always-on core.
> Still ahead: `:sealed` abilities fully static, and JIT specialization of the dispatch
> site. This section documents what is implemented today.

**Register at load time.** `*impls*` is updated with `def`, so two processes calling
`impl` *concurrently* can lose one update. Top-level `impl` forms — the normal case
— run as the module loads and are safe; this is the same configuration-time rule
telemetry's `attach`/`detach` follow. A re-registration from a *different* module is
a loud cross-module conflict (warned, last wins); from the same module it is
ordinary hot reload and stays silent.

**`next-impl` — calling the impl you shadow** (ADR-432). The precedence ladder decides
which impl dispatch runs, and every lower-tier registration is kept beside it. Inside an
`impl` method body, `(next-impl)` calls the impl one tier down for the same id, with the
same arguments — or `(next-impl x …)` with others — and falls back to the ability's
`:default` when nothing is below. So an app override wraps a library's impl instead of
copying its body:

```clojure
(impl Display shapes/circle               ; the app's override
  (->string (c) (str "⭕ " (next-impl))))  ; …around the library's own rendering
```

With nothing below and no `:default` it raises. A method whose parameters are patterns has
no argument list to pass on, so it must call `(next-impl args…)` explicitly. A method that
never mentions `next-impl` is registered exactly as written, at no cost.

Prefer `match`/`cond` when the set of cases is closed and local; reach for an
ability when third-party or later code must be able to add a case.

#### The display protocol: customizing how a record prints

The `Display` ability (ADR-171/172) is **core
and always on**. Its op **`(->string x)`** turns a value into its display string; the
`:default` impl is the native `str`. The **screen writers** (`io/write` / `io/puts` /
`io/inspect`) route a *record* through its `Display` impl out of the box — no
import, no activation step. Built-ins are unchanged and pay no dispatch cost.

```clojure
(defmodule money)             ; no (:use ability), no (:use show) — both are core
(defrecord usd (cents))
(impl Display usd
  (->string (m) (str "$" (math/->fixed (/ (get m :cents) 100.0) 2))))

(io/puts (usd 1050))          ; prints $10.50   (not #money/usd{:cents 1050})
(->string (usd 1050))         ;=> "$10.50"   ; the explicit call, for use inside str/string/interp
```

Scope is the screen printers; `str` / `pr-str` / `string/interp` stay on the native renderer
(reach for `(->string x)` explicitly there). It rides on the prelude's `*show*`
dynamic var, so `(binding (*show* nil) …)` disables it for a scope. Default record
printing is `#id{fields}` — that is intended (records print unlike bare maps); the
protocol is the per-record *override*, not a change to the default.

#### `defbehaviour`: the module-as-implementor contract

An ability dispatches on a **value**. When the unit that implements a contract is a
*module* — a live view the router calls by name, say — there is no value to
dispatch on, and that is `defbehaviour`'s job. It is **core** (`std/protocol.blsp` is
loaded in the prelude), so `defbehaviour` is available bare — no `require`, no `:use`:

```clojure
;; where the contract is declared
(defmodule views)
(defbehaviour LiveModule
  (mount (params))
  (render (model))
  (handle-event (name params model)))

;; a module that satisfies it — the header claims it, the module defines the ops
(defmodule counter-view (:use views) (:implements LiveModule))
(defn mount (params) {:n 0})
(defn render (model) (str "count: " (get model :n)))
(defn handle-event (name params model) (assoc model :n (+ 1 (get model :n))))
```

`defbehaviour` declares the ops and defines **no functions of its own** — a module
satisfies the contract by defining them itself and calling them directly. A module
claims it with `(:implements Name)` in its header, and `nest check` verifies the
module defines each declared op at the right arity. `(reflect/behaviour-ops 'Name)` exposes
the specs as data.

> **`defprotocol` / `defimpl` were retired** (ADR-168) in favour of `ability`, which
> subsumes them: they dispatched only on `type-of`, so no two records could ever
> dispatch apart. `defbehaviour` stays because a module contract is genuinely a
> different thing. Reaching for `defprotocol` now raises an unbound-symbol error
> whose hint points at `defability`/`impl`.

#### The abilities the standard library ships

Beyond the core `Display`/`Inspect`, `std/` declares these (ADR-177), and a few that serve
one module's own types (`math`'s `Numeric` and `Zero`, `tempo`'s `Spanning`, `editor/ui`'s
`Overlay`). Each is the
extension point for its module: `impl` it for your own type and that module accepts the
type with no change to it.

| Ability | Op | Module | Sealed? | What impl'ing it buys |
|---|---|---|---|---|
| `JsonEncode` | `(->json x)` | `json` | open | `json/encode` accepts your type — a record picks its wire shape, and a kind JSON has no rule for (a pid, a datetime) stops erroring. No `:default`. |
| `Port` | `(emit port s)` | `io` | open | Your value is an output port: `io/emit`, `io/with-out`/`io/with-err`, and every logger backend take it. A bare 1-arg fn is a port via the `:fn` impl. |
| `LogBackend` | `(backend-emit b record)` | `log` | open | A backend that does something other than "format one line and write it" — batching, JSON lines, sampling. Reuse `backend-passes?` for the standard level/filter gate. |
| `Response` | `(send-response r sock)` | `http` | open | A response kind with its own wire behaviour (sendfile, chunked, a 101 upgrade), including whether it closes the socket. |
| `Dependency` | `dep-kind`, `dep-resolve`, `dep-check-compatible`, `dep-lock-vec`, `dep-entry-node` | `package` | **sealed** | A new manifest dependency kind. Sealed, so `nest check` reports any op you forget. |
| `Temporal` | `(->iso x)` | `datetime` | **sealed** | ISO 8601 rendering for a calendar value. |

Sealed vs open is a per-ability judgement: `Dependency` and `Temporal` cover genuinely
closed sets and want exhaustiveness checking; the rest exist so a type the module has
never heard of can join.

`std/` also uses `defrecord` for the value types that were once plain maps told apart by
their shape — `buffer`, `queue`, `pq`, `multimap`, `datetime`/`date`/`time-of-day`, and the
four dependency kinds. Each has an identity-based predicate (`editor/buffer/buffer?`, `queue/queue?`, …) that a
look-alike map can't satisfy, and a `Display` impl, so it prints as itself rather than as
its internals. Where a library renders a *user-supplied* value into text —
`template/render`, `csv/encode`, `url/query-encode` — it goes through `->string`, so your
record's `Display` impl decides how it appears there too.

#### Polymorphism: multiple dispatch (`defmulti`)

An ability dispatches on one argument. When the choice of method depends on **more than one**
argument — the classic case is a binary operator, where `int + money` and `money + int` are
different situations — that is `defmulti`'s job ([ADR-179](decisions.md)). `defmulti` declares
a generic that dispatches on the **identity-tuple** of its arguments; `defmethod` registers a
method for an exact tuple of ids (a built-in id keyword `:int`, or a record name written like
`impl`'s):

```clojure
(defrecord money (cents))
;; `num/add` ships declared — `(defmulti num/add :commutative)`, the seam `+` consults for a
;; record — so you only add methods:
(defmethod num/add [money money] (a b) (money (+ (money-cents a) (money-cents b))))
(defmethod num/add [money :int]  (m n) (money (+ (money-cents m) n)))   ; scalar mixing, if you want it

(= (+ (money 100) (money 50)) (money 150))   ;=> true
(= (+ (money 100) 5) (money 105))            ;=> true
(= (+ 5 (money 100)) (money 105))            ;=> true   ; commutative: the [:int money] mirror is derived
```

**Resolution is total and unambiguous by construction.** A call resolves by (1) an **exact**
tuple match, else (2) a single declared **`:default`** method, else (3) a loud, structured
`no-method` error (`{:kind :%no-method :code "E0090" :message … :multi :key :have}`). There are **no partial wildcards**
and therefore **no ambiguity** — a key hits exactly one method or the default, never a
"nearest" guess. (Partial patterns + a specificity order are deliberately deferred; they are
where ambiguity lives, and Brood's flat record ids give no basis to break a tie — ADR-179.)

**Closure algebra — authoring a binary op as its upper triangle.** A `defmulti` may declare an
algebraic property that *derives* each off-diagonal method's mirror, so you write N(N+1)/2
methods instead of N² and never hit a diagonal ambiguity:

- **`:commutative`** (`+`, `*`) — registering `[A B]` also registers `[B A]` as
  `(f y x)`. Sound because `a⊕b = b⊕a`.
- **`:antisymmetric`** (`compare-to`) — the mirror is `(- 0 (f y x))`. Sound because
  `cmp(a,b) = −cmp(b,a)`.
- **none** (`-`, `/`) — no mirror; `(- 5 money)` is a `no-method`, because subtraction is
  genuinely asymmetric and the system won't fabricate a mirror the math doesn't license.

There is **no implicit coercion**: a closure derives the mirror of a method *you authored*, it
never invents a conversion — so cross-type arithmetic is explicit-per-method yet symmetric.

**`Num` and `Ord` are the built-in multimethods.** `+`/`-`/`*`/`/` route to
`num/add`/`num/sub`/`num/mul`/`num/div`, and `<`/`<=`/`>`/`>=`/`math/min`/`math/max` route to
`compare-to`, **only when a record is an operand** — pure `int`/`float`/`decimal` arithmetic
stays byte-for-byte on the kernel's fast path (records never touch it). Both are **strict**:
neither has a `:default`, so a record type must define the methods it means. `(+ (money 1)
2.5)` with no matching method is a hard `no-method`, and likewise `(< (money 1) 2.5)` — and
`(sort some-records)` — is a `no-method` until you give the record a `compare-to`. A record is
never silently ordered by its underlying map layout; define `compare-to`/`num/*` for the pairs
you actually mean. The stdlib does this for its own temporal types: `std/datetime` and
`std/tempo` register `compare-to`, so a datetime, date, time-of-day or tempo value works with
`<`, `sort` and `compare` directly (same type only — a date and a datetime do not compare,
since that would have to invent a time of day).

**Strict declarations (fail at load, and `nest check`).** A `defmethod` whose arg count differs
from its pattern length, a closure on a non-binary pattern, an unknown algebra keyword, a
method for an undeclared `defmulti`, or `:default` used as a tuple position are all **hard
errors at load** — nothing unclear is silently accepted. Redefining a `defmethod` is ordinary
hot reload (a `def` into the `*methods*` registry), visible on the next dispatch. And a
**missing method is caught statically**: `nest check` warns on a call whose argument tuple is
known but has no method and no `:default` — accounting for the closure mirror, so a
`:commutative` call in either order stays silent. An arg's identity is known from a literal, a
`defrecord` constructor, **or its inferred type** (a record-typed variable — `(let (m (usd 1))
(scale m 2.5))` is flagged). This covers **direct generic calls** *and* the **operator sugar**:
`(+ (money 1) 2.5)` and `(< money 5)` route to `num/add`/`compare-to`, so an uncovered record
operand is flagged there too (a pure-number call like `(+ 1 2)` is never touched). So an
unclear dispatch fails at type-check, not only at runtime.

Prefer an **ability** when dispatch is on one value; reach for **`defmulti`** when it depends on
a combination. `=` is deliberately *not* a multimethod — equality stays a kernel guarantee so
`hash`/sets/map-keys never desynchronise.

## Sets

A **set** is a first-class kernel value written `#{1 2 3}` — an unordered
collection of distinct elements (ADR-060). It is its own kind: `(set? s)` is true,
`(map? s)` is false, `(type-of s)` is `:set`, it prints `#{…}`, and a set is
**never** `=` to a map (even one with the same keys). Under the hood it shares the
CHAMP trie with maps (`element → true`), so it inherits structural equality
(including vector/structural elements — `#{[0 0] [1 2]}` is the natural live-cell
model for a grid) and O(log n) membership, and it is **seqable**: `count`,
`first`/`rest`, `map`, `fold`, `into`, and `vec` all treat it as its elements.
Equality is order-independent (`(= #{1 2 3} #{3 2 1})`), and a `#{…}` literal
evaluates its elements then dedups (`#{(+ 1 1) 2}` ⇒ `#{2}`).

A set is a full member of the **collection protocol**, so the ordinary prelude ops
work on one with no import: `(conj s x)` adds, `(set/disj s x)` removes,
`(contains? s x)` tests membership, `(get s x)` returns the *element* when present
(never a positional index) and `nil`/`default` when not, `(into #{} coll)` pours
and dedups, and `count`/`empty?`/`first`/`rest`/`vec`/`seq` treat it as its
elements. The kernel supplies the literal, `set?`, and the O(log n) element ops
(`%set`/`%set-add`/`%set-remove`/`%set-has?`/`%set-count`).

The **`set` library** (`(:use set)`, or a qualified `set/union` that auto-loads,
`std/set.blsp`) adds only
what's specific to sets: the constructor from a collection (`(set coll)`, which
dedups) and the algebra `union`/`intersection`/`difference`/`subset?`. Sets
deep-copy across processes like any value (`send`/`spawn` round-trip them as sets,
not maps).

> Until 2026-07-26 `conj`/`set/disj` lived in `std/set` instead, and the prelude's own
> `conj`/`into`/`get` didn't know about sets — so `(conj #{1} 2)` raised "not a
> collection", `(into #{} [1 1 2])` returned the *list* `(1 1 2)`, `(get #{10 20}
> 10)` was `nil` while `(get #{10 20} 0)` was `20`, and a `(:use set)` header
> shadowed the polymorphic `conj` so that `(conj [1 2] 3)` broke in that file. They
> are prelude functions now (ADR-156).

## Syntax

- `;` starts a line comment.
- `'expr` is shorthand for `(quote expr)`.
- `` `expr ``, `~expr`, `~@expr` are the quasiquote template markers (see
  [Macros](#macros)); `~` belongs to quasiquote **only**.
- `^expr` is a **pin** in pattern position — match the current value of `expr`
  (see [Pattern matching](#pattern-matching)). It is not metadata.
- Whitespace separates tokens; `[` `]` and `(` `)` delimit.
- A lone `.` inside a list builds a dotted (improper) tail: `(1 2 . 3)`. A `.`
  that begins an atom (`.5`, `.foo`) is not a separator.
- `{ }` is a map literal (`{key value …}`) — see [Maps](#maps). Commas count as
  whitespace, so `{:a 1, :b 2}` reads the same as `{:a 1 :b 2}`.

## Special forms

Special forms are evaluated specially (they don't evaluate all their arguments
eagerly). They are reserved names. There are exactly **eight** — the
`SpecialForm` enum in `crates/lisp/src/eval.rs`; everything else, `defmacro`
included, is a function or a macro. (`(reflect/special-forms)` is a broader,
highlighter-facing list that also names the core macros.)

| Form | Meaning |
|---|---|
| `(quote x)` / `'x` | `x`, unevaluated. |
| `(if test then else?)` | Evaluate `then` if `test` is truthy, else `else` (or `nil`). |
| `(do body...)` | Evaluate forms in order; result is the last. |
| `(def name value)` | Define/redefine `name` in the **global** environment — redefinable, the language's only mutation. |
| `(fn (params) body...)` | A lexical closure. `fn` is the only spelling — the `lambda` alias was retired (ADR-162), so `(lambda …)` is an unbound symbol whose error hints at `fn`. |
| `(let (a 1 b 2) body...)` | Sequential local bindings (each sees the previous). Brood's `let` is already sequential, so there is no separate `let*`. |
| `(letrec (f (fn ...) g (fn ...)) body...)` | Local **mutually recursive** bindings — every name is visible in every RHS (and to itself). Plain-symbol targets only; meant for fn definitions. |
| `` (quasiquote tmpl) `` / `` `tmpl `` | Template: literal except `~x` inserts a value and `~@xs` splices a sequence. |

`defmacro` is **not** among them: it is a prelude macro that lowers to
`(def name (%make-macro (fn …)))` (ADR-098) — see [Macros](#macros).
`when`, `cond`, `and`, `or`, `case`, `match`, and `comment` read like
special forms but are **prelude macros** over `if`/`do`/`let`
(`std/prelude/`), expanded once by the compile pass (ADR-022) — so the
evaluator's core stays minimal and they cost nothing extra at runtime.
(`(comment body…)` ignores its body and yields `nil` — the form-level "don't run
this", since Brood has no `#_` discard reader macro. The body is still *read*, so
it must be balanced sexps; the checker skips it, so names inside need not resolve.) `cond` is still flat test/expr pairs with **`else`** as
the catch-all (ADR-004), and `else` must be the last test. A **constant** test is refused
when the `cond` expands (ADR-518): `:else`, `true` or `42` would catch only because they
happen to be truthy, and `false` or `nil` names a clause that can never run, so neither is
a branch; `(cond … :else x)` raises with a hint to write `else`; `and`/`or` short-circuit left-to-right and return the
deciding value, each subexpression evaluated once. There is **no iteration special
form**: data is immutable and there is no local mutation (ADR-026), so loops are
expressed as recursion (proper tail calls make this O(1) stack) — or, for evolving
state, as processes (`spawn`/`receive`).

### Parameter lists

Parameter lists are written as **lists** — `(defn f (x y) …)` — because code is
made of lists (vectors `[ ]` are a data type; a vector in parameter position is an
error — `fn: parameter list must be a list, not a vector`, ADR-149). A list has three optional sections, in order:

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
an error. There is no `&key`, and there will not be (ADR-163): three or more optional
parameters take an options map, destructured with `{:keys [...]}`.

### Docstrings

A string literal as the **first body form** of a `fn`/`defn`/`defmacro` is a
**docstring** — *when more body follows it*. A function whose body is a lone
string returns that string (the CL/Elisp rule), so it isn't documentation:

```clojure
(defn square (x)
  "Return x times itself."   ; docstring (more body follows)
  (* x x))

(reflect/doc square)                 ;=> "Return x times itself."

(defn greeting (who) "hello") ; lone string → return value, NOT a docstring
(reflect/doc greeting)                ;=> nil
(greeting 'x)                 ;=> "hello"
```

The docstring is stored on the closure and read with `(reflect/doc f)` (below); it
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

For purely side-effecting iteration there is one form, `doseq`. It takes `for`'s
bindings (several pairs, `:when` guards, destructuring), collects nothing and returns
`nil`. A counted loop iterates a `range` (ADR-674):

```clojure
(doseq (i (range 3)) (io/write i " "))    ; prints "0 1 2 "
(doseq (x (list :a :b))                   ; runs the body for each element
  (io/puts (->string x)))                 ; prints "a" then "b"
```

There is no `dotimes`, `dolist` or `unless`: ADR-674 removed them. A call of one is
unbound, and the error names its replacement: `doseq`, `doseq` over a `range`, and
`(when (not test) …)`.

**Comprehensions** build a value from the same walk. `for` collects a list of its
body's values over one or more bindings (the last varies fastest), with `:when`
guards; a trailing **`:into coll`** collects with `conj` onto `coll` instead, so the
target's kind decides the result; and **`fold-for`** names an accumulator as its first
pair and makes the body the step (ADR-377 — Racket's `for/fold`):

```clojure
(for (x (range 5) :when (math/even? x)) (* x x))   ;=> (0 4 16)
(for (x [1 2 3] :into []) (* x x))                  ;=> [1 4 9]        ; a vector appends
(for (x [1 2] :into {}) [x (* x x)])                ;=> {1 1, 2 4}     ; a map takes [k v]
(for (x [1 2 -1] :into #{}) (* x x))                ;=> #{1 4}         ; a set dedups
(fold-for (sum 0 x [1 2 3]) (+ sum x))              ;=> 6
(fold-for (m {} x [:a :b] :when (not (= x :b))) (assoc m x 1))   ;=> {:a 1}
```

All three are macros over one expander — a nested `fold` per binding, an `if` per
guard — so a comprehension is one pass and tail-recursive; `for` without `:into`
builds in reverse and reverses once at the end.

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
;; defn is itself a macro, defined in the prelude — this is its shape (`defn` is reserved,
;; so it is spelled `my-defn` here):
(defmacro my-defn (name params & body)
  `(def ~name (fn ~params ~@body)))

(my-defn square (x) (* x x))     ; expands to (def square (fn (x) (* x x)))
(square 4)                       ;=> 16

;; your own:
(defmacro unless2 (c & body) `(if ~c nil (do ~@body)))
(unless2 false (io/puts "ran"))

;; inspect an expansion without running it:
(reflect/macroexpand-1 '(defn f (x) x))   ;=> (def f (fn (x) x))
```

### Macro hygiene — automatic

Macros are **hygienic by default**. A binder a backtick template introduces is
automatically alpha-renamed to a fresh symbol, so a plain literal binder can neither
capture nor be captured by caller code spliced in with `~`/`~@` — no `reflect/gensym`, no `#`.
That holds at every binding site of the one pattern grammar: `let`/`letrec`/`fn` binders
and their destructuring patterns, `match`/`receive` clauses (guards, `or` alternatives,
pins), and the standard binding macros — `when-let`, `if-let`, `for`, `fold-for`, `doseq`,
`with`, `as->`, `try`'s `catch`, and the params of `defn`, `defmethod` and
`impl`. A typed binder keeps its annotation (`x:int` becomes `x__12:int`), and a renamed
parameter still reads as written wherever a parameter list is shown — an arglist, a doc,
an arity error say `(a)`, not `(a__12)`:

```clojure
(defmacro my-or (a b)
  `(let (r ~a)            ; plain `r` — auto-renamed to a fresh symbol
     (if r r ~b)))

(let (r 1) (my-or false r))   ;=> 1  ; the caller's `r` is not captured
```

The rename is scope-aware (it touches only the references a binder actually binds)
and fresh per expansion (so nested expansions of one macro, `(m (m x))`, get
distinct binders). Free references in a template (`helper`, `map`) are handled
separately — they **auto-qualify** to the macro's defining namespace — so both
halves of hygiene are automatic.

**Opting a binder OUT — anaphora.** When a template *deliberately* introduces a
name for the caller's spliced code to reference (an anaphoric macro like `aif`),
write it as an unquoted quoted symbol, `~'name`, which emits the literal name:

```clojure fragment
(defmacro aif (test then else)
  `(let (~'it ~test)          ; ~'it stays the literal `it`
     (if ~'it ~then ~else)))

(aif (find x) (use it) :none)  ; the caller's `it` refers to the binding
```

**`x#` and `reflect/gensym` still work** (now redundant): a symbol ending in `#` inside a
template still becomes a fresh gensym (the Clojure shorthand), and `reflect/gensym`
remains available for a fresh symbol outside a template. New code needs neither.

**What stays literal**, so opt into `#`/`reflect/gensym` there if capture is a concern:

- a binder introduced by **your own binding macro** used inside a template — the rename
  runs before the macros in a template expand, so it knows the standard library's shapes
  but cannot know yours;
- a **computed** binder, `(fn ~params …)` or `(let ~bindings …)` — the macro builds those
  names itself, so it names them fresh;
- a **`{:keys [a]}`** name — the names are the keys looked up, so a renamed `a` would read
  `:a__12`. Bind with `{:a a}` (which requires the key) when that matters.)

### Macros that write macros — nested quasiquote

A template may hold another template. Each `` ` `` raises the **level** by one and each
`~`/`~@` lowers it by one; only a hole that brings the level to zero is filled when the
outer template is built. A deeper hole is copied as written, to be filled when the inner
template runs (ADR-503):

```clojure
`(list `(vector ~(+ 1 2)))           ;=> (list (quasiquote (vector (unquote (+ 1 2)))))
(let (x 'y) `(list `(vector ~~x)))   ;=> (list (quasiquote (vector (unquote y))))
(let (x 5) `(list `(vector ~'~x)))   ;=> (list (quasiquote (vector (unquote (quote 5)))))
```

What it is for: a macro whose expansion defines another macro. There are two moments of
expansion — when `def-tagged` runs (it fills `~name` and `~tag`), and when the macro it
defined runs (it fills `~body`). `~'~tag` fills `tag` at the first and keeps the value as
a constant for the second:

```clojure
(defmacro def-tagged (name tag)
  `(defmacro ~name (body) `[~'~tag ~body]))

(def-tagged as-ok :ok)   ; defines the macro `as-ok`
(as-ok (+ 1 2))          ;=> [:ok 3]
```

The rest of the template rules hold at every level:

- a **free name** in the inner template means the defining module's binding, so the
  generated macro works from any module;
- an inner template's **binders** belong to the macro it builds: they are renamed each time
  that macro expands, not once when the outer one did. A hole back to the outer level
  (`` `(let (x 1) `(f ~x)) ``) reads the outer template's binders;
- an **`x#`** belongs to the template that directly holds it: `` `(a `(b c#)) `` leaves
  `c#` for the inner template, while `` `(a `(b ~c#)) `` takes the outer one's gensym.

The `->` and `as->` threading macros are also defined in the prelude. There is only
**one** pipe: every collection function takes its collection first (ADR-308), so a
thread-*last* variant would have nothing to thread (`->>` and friends were deleted):

```clojure
(-> 5 (- 1) (* 2))            ;=> 8     ; (* (- 5 1) 2)   thread as FIRST arg
(as-> 5 $ (+ $ 1) (* $ 2))    ;=> 12    ; bind $, thread into ANY position
```

The whole rule is one sentence (ADR-507): **the subject comes first, a callback comes last,
and only the callback's own arguments may follow it.** The subject is what the call is
about — a collection, a map, a process, a seed — so `(seq/iterate-times 1 4 f)` and
`(map/merge-with [a b] +)` read in that order. Two things follow from the wording:

- arguments *for* the callback come after it, since they belong to it: `(update m :n + 5)`
  calls `(+ old 5)`, and `map/update-in`, `proc/offload` and `partial` do the same;
- when the call transforms or runs a function, that function **is** the subject and comes
  first: `(combinator/fnil f 0)`, `(gen/start server state)`.

An optional slot goes before the callback, as a second arity — `(reduce coll f)` and
`(reduce coll init f)` — never after it. A callback's *own* parameters keep their order:
a reducer is `(fn (acc x))` (ADR-308).

A step form naming **`$`** receives the threaded value at that position instead of first
(ADR-303). The stdlib puts the subject first, so a pipeline over it needs no `$`; the
hole is for a step that wants the value somewhere else — an operator's second operand, a
separator, an element of a literal:

```clojure
(-> 1 (- 10 $))                                  ;=> 9     ; not -9
(-> ", " (string/join ["a" "b"] $))              ;=> "a, b"
(-> 5 (+ 1 (* 2 $)))                             ;=> 11    ; found at any depth
(-> 3 [$ $])                                     ;=> [3 3] ; and in vector/map literals
```

A form with no `$` threads exactly as before, so this is purely additive. The value is
bound **once** and the binding substituted, so a step may name `$` repeatedly without
re-evaluating it — `(-> (expensive) (+ $ $))` calls `expensive` once. A quoted `'$` is
the symbol, not a hole. `cond->` inherits it.

The **conditional threading** macros build on those, plus `doto`:

```clojure
(cond-> {} true (assoc :a 1) false (assoc :b 2))  ;=> {:a 1} ; apply a step only when its guard holds
(doto (table/new) (table/put :a 1) (table/put :b 2))  ; run forms for effect, return the value
```

**`ok->`** is the **failure** pipe: the first step yielding a failure short-circuits, and
that failure falls out carrying its message, so a pipe does not have to end in
`(if (failure? $) $ (step $))`. `nil` is not a sentinel — absence and failure are separate
channels (ADR-310) — so `ok->` threads straight through a nil.

```clojure
(ok-> "12" string/->number inc)          ;=> 13
(failure? (ok-> "q" string/->number (* 2) (+ 5)))  ;=> true   ; neither * nor + ran
(ok-> nil identity)                      ;=> nil           ; nil is not a sentinel
```

A plain `->` does **not** stop: no primitive absorbs a failure, so `(-> "q"
string/->number inc)` raises. Stopping is something you say (ADR-315) — `ok->` here,
`with` for a chain of named steps, and a fold stops once its accumulator is a failure.

There is **no `some->`** (deleted by ADR-315). It stopped on `nil`, which since ADR-310
means only "the lookup found nothing" and is an ordinary value everywhere else in the
language — a pipe for a channel that is not one. `map/get-in` covers the nil-chaining it
existed for.

`each` applies a function to
each item for effect (`(each xs io/puts)`, the function form of `doseq`).

`debug/tap` and `then` are the single-**function** pipe helpers, where `doto`/`->` splice *forms*: `(debug/tap x f)` calls `(f x)` for its side effects and returns
`x` unchanged (a probe — `(-> v (debug/tap log) (process))`); `(then x f)` returns `(f x)`, letting a
`->` step call an arbitrary function on the threaded value instead of splicing it as a first
argument (`(-> v (then (fn (n) (g n other))))`).

**Binding-conditionals** bind, test the *source* value, then branch (the target may
destructure):

```clojure fragment
(if-let (v (get m :k)) (use v) :absent)   ; bind v; take `then` when truthy, else `else`
(when-let (v (get m :k)) (use v))         ; body only when truthy
```

**`with`** — the **failure `let`**. Bindings read exactly like `let`'s; the first value
that is a failure short-circuits the whole form to that failure, so the steps after it
never run:

```clojure fragment
(with (account (lookup user)
       card    (payment-method account)
       receipt (charge card 10))
  receipt)                                ; lookup's failure falls out; charge never runs
```

The bindings are **one flat list**, exactly like `let`'s — not a list per pair. Writing
`(with (a e1) (b e2) body)` binds only `a`; `(b e2)` then becomes a body form that *calls*
`b`, which surfaces as a puzzling `unbound symbol: b`.

Where `ok->` threads a value, `with` *binds* one — which is why `with` is the mechanism
that reaches a chain of **your own** functions. A failure threaded into a user-defined step
does not stop it: the body runs, branches on the failure, and answers with its own, losing
the cause. The binding target may destructure (`(with ([a b] (pair-of x)) …)`); the value
is bound to a temporary and tested first, so a failure never reaches a pattern that could
not match it.

This is deliberately **not** a pattern-per-step `with` (`[:ok a] <- expr`), which Brood had
until ADR-315. That shape is needed only where there is no failure kind — the tuple *shape*
carries the signal, so every step spells out a pattern for "kept going" and an `:else`
section re-matches the shapes that did not. Brood has a failure kind, so both are redundant: what falls out is
always a failure, and `if`/`match`/`failure?` handle it at the call site.

**Local loops** — there is **no `loop`/`recur`**; Brood has proper tail calls, so a
self-contained loop is a `letrec`-bound closure you call by name (it closes over the
enclosing scope, so you thread only the *changing* state, and the tail call is O(1)
stack):

```clojure
(letrec (go (fn (n acc) (if (= n 0) acc (go (- n 1) (+ acc n)))))
  (go 100000 0))                                ;=> 5000050000   ; O(1) stack
```

**`string/interp`** — string interpolation. `(string/interp "…{expr}…")` splices each `{expr}` hole's
value between the literal text and lowers to a plain `(str …)` (no runtime cost);
`{{`/`}}` are literal braces and braces nest inside a hole:

```clojure
(def a 3)
(def b 4)
(def name "ada")
(string/interp "sum={(+ a b)} for {name}")   ;=> "sum=7 for ada"
```

**`debug/spy`** — a homoiconic tree-tracing debug macro (ADR-173). `(debug/spy expr)` evaluates `expr`, traces **every** evaluated subexpression's
value in evaluation order, and returns the value **unchanged** — so it's referentially
transparent: wrap or drop `debug/spy` around any form with no behavioural change.

```clojure fragment
(debug/spy (+ (* a 2) (f 2)))     ; a=10, f adds 5
;; stderr:
;;   spy: (+ (* a 2) (f 2))
;;       (* a 2) => 20
;;       (f 2) => 7
;;     (+ (* a 2) (f 2)) => 27
;;   => 27
;; returns 27
```

It fully macroexpands the form and instruments each evaluated position **in place**, so
laziness is preserved (an untaken `if` branch or a short-circuited `and` tail never
traces) and a **pipeline needs no special case** — `(-> x f g)` expands to `(g (f x))`
and each stage traces as an ordinary call. `fn` bodies and quoted data are left opaque.
Trace entries flow through the swappable **`*spy-sink*`** (a `defdyn`) — the default
prints the indented tree to stderr, but a host can `binding` it to a collector and
consume the trace as data (`{:spy :node :form … :value … :depth …}` maps), or to a
no-op to silence it without editing code:

```clojure fragment
(binding (*spy-sink* (fn (entry) nil)) (debug/spy (heavy-computation)))  ; value, no output
```

> Macros are hygienic both ways automatically: a template's introduced `let`/`fn`
> **binders** are alpha-renamed so they can't capture spliced caller code (ADR-066
> amendment; opt a binder out with `~'name`), and its *free* references
> **auto-qualify** to the macro's defining namespace (ADR-066 α), so a macro
> expands correctly when used in another namespace without hand-qualifying. See
> the "Macro hygiene" section above and spec §7.

## Pattern matching

Pattern matching, with **one pattern grammar reused at every
binding site**: `match`, refutable `let`, and `fn`/`defn` clauses. The compiler
is written in Brood (`std/prelude/match.blsp`) — no new special form. For the full
design and rationale see [pattern-matching.md](pattern-matching.md).

### The grammar

| Pattern | Matches / binds |
|---|---|
| `_` | anything; binds nothing |
| `x` | anything; **binds** `x` (a repeated `x` is an equality constraint) |
| `42` `"s"` `:k` `true` `nil` | a literal, compared with `=` |
| `'sym` | the literal symbol `sym` |
| `^expr` | the current value of `expr` (a *pin*) |
| `(p1 p2 …)` | a list of that exact length, element-wise |
| `(p1 & rest)` | head(s) + the tail bound to `rest` |
| `[p1 p2 …]` | a vector of that exact length — the **tagged-data / tuple idiom** |
| `{:keys [a b] :or {a 0}}` | a **map** — binds each `:keys` symbol to the same-named keyword's value (nil if absent, or the `:or` default); fails if the value isn't a map. `{}` matches any map. |
| `(bytes seg…)` | a **`bytes` value**, destructured segment-by-segment — bit syntax (see below) |
| `(or p q …)` | any alternative — first match wins; every alternative must bind the same names |
| `(and p q …)` | every pattern, against the same value — the capture-while-destructuring (`:as`) idiom |
| `{:k p}` | a map with key `:k` **present**, whose value matches `p` (nests to any depth) |
| `(record name {…})` | a **`defrecord` value** whose nominal id is `name` (bare → current ns, or `mod/name`), then the map pattern `{…}` against its fields (`{:k p}`/`:keys`/`:or` all compose); the `{…}` is optional (id-only). A plain map or non-record fails |

Patterns nest to any depth. **The one trap:** a bare symbol *binds* (and
shadows) — it does **not** test against a same-named value. Match a known value
with a keyword (`:ok`), a quoted symbol (`'none`), or a pin (`^x`).

### Alternatives, conjunctions, and map sub-patterns

`(or p q …)` matches if **any** alternative matches, first one wins:

```clojure fragment
(match code
  ((or 200 201 204) :success)
  ((or 301 302)     :redirect)
  (_                :other))
```

Alternatives may bind, but **every alternative must bind the same names** — else the
body could reference a name only some of them bind, and which one would depend on the
input (`(or [1 x] [2 x])` is fine; `(or a 2)` is a compile error).

`(and p q …)` matches when **every** pattern matches the same value, left to right,
with later patterns seeing what earlier ones bound. That is how you capture the whole
while destructuring it — Clojure's `:as`, Rust's `x @ pat`:

```clojure fragment
(match msg
  ((and whole {:keys [kind]}) (log kind whole)))
```

A map pattern's **explicit keys are sub-patterns**: `{:status 200}` requires the key
to be *present* and its value to match, nesting to any depth
(`{:user {:id id}}`). Note the deliberate split:

| Spelling | Semantics | If the key is absent |
|---|---|---|
| `{:keys [a b]}` | Clojure **destructuring** | binds `nil` (or the `:or` default); never fails |
| `{:k pat}` | **map pattern** (a refutable match) | the clause **fails** |

Both may appear in one pattern (`{:type :circle :keys [radius]}`), and `{}` still
matches any map.

**Two spellings stay rejected** rather than reinterpreted (ADR-152) — each would
otherwise silently mean something else:

- **`(not …)`** — a negative match binds nothing, which makes it a *guard*: write
  `(x :when (not …) …)`. As a plain list pattern it would bind a variable named `not`.
- **`:as` in a map pattern** — now that explicit keys are sub-patterns, `{… :as m}`
  would read as "this map must have an `:as` key". Use `(and m {…})`.

> Until 2026-07-26 `or`/`and`/`{key subpattern}` were all *silent misreads*:
> `(match 2 ((or 1 2) :hit) (_ :miss))` answered `:miss` (binding a variable named
> `or`), and a map pattern's unknown keys were ignored, so `{:a v}` degenerated to
> "is it a map?" — matching anything, binding nothing. They were made hard errors in
> ADR-156 and implemented in ADR-160.

### Bytes patterns (bit syntax)

A `(bytes seg…)` pattern destructures a `bytes` value left-to-right. Each segment consumes bytes:

| Segment | Consumes / binds |
|---|---|
| `7`, `#b"GET "` | those exact byte(s), by content |
| `x` | one byte, bound as an int 0–255 (`_` skips one) |
| `(x n)` | `n` bytes as a sub-`bytes` value; `n` is an int **or an earlier binding** (dynamic size); `(_ n)` skips |
| `(x :u16)` | a typed integer: `:u8`/`:u16`/`:u32`/`:u64` unsigned, `:i8`/`:i16`/`:i32`/`:i64` signed two's complement — **big-endian by default**, explicit `:u16-be`/`:u16-le` (etc.) variants; `(_ :u32)` skips the width |
| `& rest` | the remaining bytes (must be last) |

Without a trailing `& rest` the pattern requires the bytes be consumed
*exactly*; a too-short or too-long value falls through to the next clause. A
repeated binder is an equality constraint, as everywhere else. A full-range
`:u64` read past `i64` widens to a big integer transparently.

```clojure fragment
;; TLV: a u16 length prefix drives a dynamic-size payload
(match frame
  ((bytes (len :u16) (payload len) & rest) (handle payload rest))
  (_                                       :short))

;; a header: magic byte, kind, big-endian length, then exactly len bytes
(match pkt
  ((bytes 127 kind (len :u16) (body len)) [kind body])
  (_                                      :bad-frame))
```

The same reads and writes are available as plain functions for offset-based
parsing: `(bytes/uint bs off n)` / `bytes/uint-le` / `bytes/int` /
`bytes/int-le` (n = 1–8 bytes), and the encoders `(bytes/int-> v n)` /
`bytes/int-le->` (truncating to `n` bytes, the bit-syntax convention — so
`(bytes/int-> -1 2)` is `#b"\xff\xff"`).

### `match`

Clauses are **wrapped** `(pattern [:when guard] body…)`; the first whose pattern
(and guard) matches runs its body.

```clojure fragment
(match msg
  ([:say text]      (io/puts text))
  ([:add a b]       (+ a b))
  ((x & xs)         (str "head " x ", rest " xs))
  (n :when (int? n) (handle-int n))
  (_                :unknown))          ; explicit catch-all
```

A `match` in tail position is TCO-safe (loops and receive loops won't overflow).
No clause matching **raises** an error of kind `:match` (ADR-501) —
`{:kind :match :message … :context <form> :value <value> :patterns <patterns-tried>}` —
which a `catch` pattern can pick out. Add a `_` clause to make a match total:

```clojure fragment
(try (match resp ([:ok v] v))
  (catch {:kind :match :value val} (recover val)))
```

**Function-head guards.** The same `:when` guard works on a `fn`/`defn` **clause
head** (ADR-226) — the clause applies only when its guard is truthy, else dispatch
falls through to the next clause:

```clojure
(defn classify
  ((n) :when (< n 0) :negative)
  ((n) :when (= n 0) :zero)
  ((n) :positive))
```

The guard sees every parameter the clause binds, `&optional` and rest ones included. A
guarded fn dispatches through the match engine (so tail calls are still TCO-safe, and a
guard that fails on the last clause raises a `:match` error like any unmatched `match`).
A single-clause `fn` takes one too, first in its body (after a docstring, if any):
`(defn half (n) :when (math/even? n) (/ n 2))` (ADR-619).

A guard is **any expression** — Brood does not restrict it to a whitelist of guard-safe operators: it may call your own functions, closures, `let`, or any
computation that yields a truthy value. Two consequences: a guard **can** have side effects,
be slow, or not terminate (discipline keeps it pure, not the compiler); and a guard that
**throws propagates** the error rather than silently falling through to the next clause. For
"on error, try the next clause," guard the guard: `:when (try (test x) (catch _ false))`.

### `case` — literal dispatch

`case` dispatches on a **value** against literal tests, written as flat
`test result` pairs (`cond`'s shape) with a lone trailing form as the default:

```clojure fragment
(case status
  :ok      (render body)
  :missing (render-404)
  :error   (render-500)
  (render-unknown status))          ; lone trailing form = default
```

It is sugar over `match`, but not a synonym for it — the difference is what it
*refuses*. A `case` test must be a literal (keyword, int, float, decimal, string,
bool, `nil`, or a quoted symbol); a **bare symbol is an error**, because in `match`
it would silently *bind* instead of comparing. So `case` is the spelling to reach
for whenever every arm is a constant, and `match` the one for shapes, guards, and
binding. Anything richer than a literal — destructuring, a guard, alternatives — is
rejected with a hint naming `match`. With no default, no match raises
a `:match` error with `:context :case`, exactly as `match` does, and the
[exhaustiveness lint](#type-annotations) covers a `case` over a declared literal
type just as it covers a `match`.

**`condp`** is `cond` with the predicate factored out — `case` compares literals, this
asks a function (ADR-379). Its subject comes first, as in `case` and `match`, then the
predicate (ADR-507). It evaluates the subject once and takes the first clause whose
`(pred expr test)` is truthy — **value first**, the way every data-first Brood predicate
reads — so each clause reads in the order its test runs:

```clojure
(condp 15 < 10 :small 100 :medium :large)                            ;=> :medium
(def line "# Title")
(condp line string/starts-with? "#" :heading "-" :item :text)        ;=> :heading   ; a lone trailing form is the default
```

**`if-some` / `when-some`** are `if-let` / `when-let` testing for *presence* rather than
truth, so a bound `false` takes the then-branch — the binding forms for a lookup whose
value may legitimately be `false`: `(if-some (flag (get config :verbose)) flag :unset)`.

### Refutable / destructuring `let`

A `let` binding target may be a pattern; it's a refutable bind (Brood's `=`) that
raises on mismatch. Bindings stay sequential, freely mixed with plain symbols:

```clojure fragment
(let (a 1                    ; plain symbol (unchanged)
      [:ok v] (fetch key)    ; refutable: raises if fetch isn't [:ok _]
      (x & xs) (range 10))   ; destructure a list
  (use a v x xs))
```

### `fn` / `defn` clauses

`fn` is **multi-clause** when every form after it is a clause `(param-list body…)`. The
clauses are tried **in order** and the first that matches runs (ADR-619, below), whether a
clause is told apart by its argument count or by the shape of its arguments:

- **By argument count** (Clojure-style multi-arity) when the heads are *arity
  clauses* — plain-symbol params, optionally with `&optional` / `&` rest. Each arm
  binds its params *directly* (no rest-list), so it's as cheap as a single-clause fn —
  this is how the prelude's variadic `+`/`-`/`<`/`=` stay fast and stay Brood (ADR-047).
- **By pattern** when a head contains literals or destructuring — the first matching
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
  ((a & more) (+ 1 (count more))))

(defn fac                               ; multi-PATTERN: same arity, dispatch by shape
  ((0)  1)
  ((n)  (* n (fac (- n 1)))))

(defn area ([x y]) (* x y))             ; single-clause, tuple-destructured param
(defn move (p [dx dy] &optional (n 1))  ; patterns coexist with &optional / & rest
  …)
```

**One head grammar, one dispatch rule (ADR-619).** Every head — single-clause,
multi-clause, `defmacro`, and the clauses of a pattern-dispatched `fn` — is a parameter
list built from the same slots:

- **required** slots: each a pattern (a plain name is the pattern that matches anything);
- after `&optional`, **optional** slots: `name`, `(name default)`, or `(pattern
  default)` — the default is evaluated only when the argument is absent, and sees the
  names bound before it;
- after `&`, a **rest**: a name, or a pattern over the list of remaining arguments;
- a **`:when` guard** first in the body.

**Clauses are tried in order, and the first that matches runs** — by argument count and
by shape alike, so arity clauses and pattern clauses mix in one `defn`:

```clojure
(defn rect
  ((:unit) 1)
  ((w &optional (h w)) (* w h)))
(rect :unit)    ;=> 1
(rect 3)        ;=> 9
(rect 3 4)      ;=> 12
```

**A clause an earlier one shadows is a compile error.** When an earlier clause takes
every call a later one would — it accepts every argument count the later one does, has no
guard, and every slot is a plain name — the later clause could never run, so the `fn` is
refused, naming both. `((x &optional y) …)` before `((x) …)` is refused; written the
other way round it is fine, and `(f 1)` runs `((x) …)`. That is what keeps clause order
from changing a program's meaning unnoticed. (Until ADR-619 arity clauses picked the most
specific clause whatever the order, while pattern clauses went in order; now there is one
rule.) Partial overlap is allowed and runs the first: `((n) :when (> n 0) …)` before
`((n) …)`, or `((0) …)` before `((n) …)`.

The same slots work in a `match` list pattern: `(a &optional (b 0) & more)` matches a list
of one or more elements, and binds `b` to `0` when there is no second one.

Parameter lists stay **lists** (ADR-010), so a single tuple parameter must be
wrapped: `(defn g ([x y]) …)` is one 2-tuple param, while `(defn g (x y) …)` is
two params.

**A macro takes clauses the same way**, since `defmacro` forwards to `fn`: one clause
  per accepted shape of the arguments, and a final catch-all that raises the macro's own
  message instead of a generic match error:

  ```clojure
  (defmacro swap-pair
    (([a b])   [b a])
    ((& args)  (error "swap-pair: expected one [a b] vector literal, got " (pr-str args))))
  ```

  A macro's error is placed at the macro call, never at the top-level form around it
  (ADR-618). To point at the sub-form it is complaining about, a macro raises with
  `(reflect/error-at form & parts)`, which reads like `error` but takes its position from
  `form`. Only lists carry a reader position, so a symbol, a number or a vector leaves the
  error at the call:

  ```clojure
  (defmacro needs-pair (form)
    (if (and (pair? form) (= (count form) 2))
      `(quote ~form)
      (reflect/error-at form "needs-pair: expected (a b), got " (pr-str form))))
  ```

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
higher-order context (`(map … (fn ([k v]) …))`) remain idiomatic. See
[brood-for-claude.md](brood-for-claude.md) §"Style — lists for code, vectors
for data" for the full rule.

## Errors

Brood has **two** channels, and which one a function uses says what kind of thing went
wrong (ADR-310):

| what happened | mechanism | who deals with it |
|---|---|---|
| **a bug** — wrong type, wrong arity, unbound symbol, a broken invariant | **raises** | `try`/`catch`, else the supervisor |
| **the unexpected** — something failed at runtime that nothing planned for | **raises** | let it crash; the supervisor restarts |
| **a known failure** — "this text is not a number", "these bytes are not base64" | **returns a `failure`** | the caller, right there |

They cannot be confused, because they do not travel the same way: one is handed back as
a value, the other unwinds. So a `try` only ever catches a bug or the unexpected, which
is what a `try` should be for.

### Known failures — a returned value

A function that cannot interpret its input returns a **`failure`**: its own kind, not
`nil`, carrying a message that names the input.

```clojure
(string/->number "42")                    ;=> 42
(string/->number "abc")                   ;=> #failure{:message "string/->number: not a number: \"abc\""}
(failure? (string/->number "abc"))        ;=> true
(error-message (string/->number "abc"))   ;=> "string/->number: not a number: \"abc\""
```

**A failure is truthy**, like every value that is not `nil` or `false`. So it never
quietly disappears into a default — you handle it, or it flows onward and the next
operation reports it:

```clojure
(def s "abc")
(failure? (or (string/->number s) 0))     ;=> true    ; the FAILURE, not 0 — it is truthy
(if (string/->number s) :yes :no)         ;=> :yes    ; there IS a value, just not a number
(count (seq/keep ["1" s] string/->number))  ;=> 2    ; keeps them: `keep` drops only nil

(let (n (string/->number s))              ; the way to default, said out loud
  (if (failure? n) 0 n))                  ;=> 0
```

**It is not `nil`, though**, and that is the point: `nil` means *absence* — a lookup
found nothing — and accessors keep it (`get`, `nth`, `os/env`, `first` of an empty
list). A failure means *this could not be produced*, and says why. Two different
questions, two different answers.

Write one with `failure`, in the same place you would have written `error`:

```clojure
(defn parse-range (s)
  (match (string/split s "-")
    ((a b) [(string/->number a) (string/->number b)])
    (_ (failure "parse-range: not a range (a-b): " (pr-str s)))))
```

Reach for `failure` when the caller asked something whose honest answer is "that cannot
be interpreted". Reach for `error` when the caller made a mistake — that is a bug, and
it should unwind.

When a caller needs to tell *which* failure it got, without parsing the message, attach
fields with `failure/with` and read them back with `fields` (ADR-617). The message is
still the concatenated parts; the fields sit beside it, and may not name `:message`:

```clojure
(def path "/x")
(def missing (failure/with {:status 404} "not found: " path))
(error-message missing)            ;=> "not found: /x"
(get (fields missing) :status)     ;=> 404
(fields (failure "nope"))          ;=> {:message "nope"}
```

A failure is still not a collection — `get` on one raises — so `fields` is the one way
in, after `failure?` has said it is one.

### Raising — for bugs and the unexpected

Raise with `error` (a formatted message) or `throw` (any value), and handle with
`try`/`catch`:

```clojure fragment
(try
  (risky)
  (catch e
    (io/puts "failed:" (error-message e))
    :recovered))

(error "bad index: " i)             ; raise an error map {:kind :error :message "bad index: 7" …}
(throw :boom)                       ; raise an arbitrary value, as it is
```

**Every error the language raises is an error map** (ADR-501):
`{:kind <keyword> :message <string> [:code :file :line :col :hint :trace]}`. A built-in
error's kind names what went wrong (`:type`, `:arity`, `:unbound`, `:runtime`,
`:contract`, …); `error` raises kind `:error`, `assert` kind `:assert`, and a pattern that
matches nothing — a `match` or `case` without a catch-all, a refutable `let`, a pattern `fn`
— kind `:match`, which also carries `:context` (`:match`, `:let`, `:fn`, …), `:value` and
the `:patterns` tried:

Calling a value that is not a function names what was called when the head is a local, and
says so when that local shadows a root function — the reflex worth naming, since the local
won and the function was never called. Every tier gives the same message:

A value that is not a function, called from library code — `(fold [1 2] + 0)` hands `fold` a
`0` where the reducer belongs — gets a hint about argument order (the collection first, the
function last) rather than the `f(x)` one. A pattern function names itself when nothing
matches, and a call no clause takes by count says so in arity terms:
`pf: no clause accepts 2 argument(s) (arities: 1)`. An error a program's run raises inside the
standard library is reported at the program's own call, naming the library function
(`math/abs: <: expected number …`), with the library's line kept in the trace.

```clojure
(defn f (list) (list 1 2))
(try (f [1]) (catch e (:message e)))   ;=> "cannot call non-function: `list` is [1]"
(try (f [1]) (catch e (:hint e)))      ;=> "`list` here is a local binding, which shadows the function `list`. Rename the local to call the function (or write `/list`, which always means the root binding)."
```

```clojure
(try (match 3 (1 :one)) (catch e (:message e)))   ;=> "match: no clause matches 3 — tried 1"
(try (match 3 (1 :one)) (catch e (:value e)))     ;=> 3
(try (error "bad " 42) (catch e (:kind e)))        ;=> :error
```

`throw` is the one exception: it raises its argument **as it is** — a keyword for a
non-local exit, or a program's own error, which it writes as a map with `:kind` and
`:message` so it reads like the rest.

**`(assert test message…)`** states an invariant: when `test` is falsy it raises an
`:assert` error with the form's own text, followed by any `message` parts; otherwise it returns
`test`'s value, so it can wrap the expression it checks. The message parts are evaluated
only when it raises. It is always on — there is no build that strips it.

```clojure fragment
(assert (> n 0))                    ; raises "assert failed: (> n 0)" when n is 0
(assert (> n 0) "got " n)           ; "assert failed: (> n 0): got 0"
(let (conn (assert (lookup id)))    ; the value of (lookup id), or a raise
  …)
```

A final **`(finally cleanup…)`** clause runs `cleanup` after the body — and after
the handler, when there is a `catch` — whether the try completed or threw, then
yields the try's value or rethrows what escaped (ADR-306). `cleanup`'s own value
is discarded; a throw from `cleanup` replaces the pending one. It is the
`unwind-protect`/`dynamic-wind` of the other Lisps, for a resource that must be
released on the way out of a handler that stays alive — a port, a `table` row, a
lock held in a `gen` server — where letting the process die is not an option:

```clojure fragment
(try
  (io/emit port (render frame))
  (catch e (log/warn "render failed" {:error (error-message e)}))
  (finally (release port)))
```

`finally` does **not** run on an untrappable kill (`(exit pid :kill)`): a kill is not an error and unwinds no handler. A rethrown built-in
error is rebuilt from its map by `throw` (`kind`, message, position, hint and
`:trace` restored), so what escapes a `finally` — or a `(catch e (throw e))` —
prints and propagates exactly as the original did rather than as a printed map. A
rethrown USER error keeps where it was raised too: `catch` binds its payload as
thrown, and the runtime remembers the position and trace of a caught payload that
lives on the heap (a string, a map, …), so `throw` of that same value raises it from
its original site (ADR-427). A keyword or number has no identity to remember; it is
located at the rethrow.

**`catch` takes a pattern** — the one pattern grammar, with an optional `:when` guard —
and a `try` may have several, together after the body. The first whose pattern matches
what was raised runs, with the pattern's bindings; when none matches, the error
propagates, raised again from where it was first raised. A symbol matches anything, so
`(catch e …)` catches everything:

```clojure
(try (/ 1 0)
  (catch {:kind :type} :type)
  (catch {:kind :runtime :message m} m))           ;=> "division by zero"
(try (throw :found) (catch :found :got-it))        ;=> :got-it
(try (error "x") (catch e :when (= (:kind e) :error) (:code e)))   ;=> "E0090"
(try (error "x") (catch (and e {:kind :error}) (error-message e)))  ;=> "x"
```

There is no exception class — Clojure's `(catch Type e body…)` is rejected with a hint,
since reading it as a pattern would bind the *class name* and evaluate the intended binder
as a statement (a wrong program, not an error). A `try` with no `catch` is just a `do`.
Under the hood `throw`, `%raise` and `%try` are primitives and `try`/`catch`/`error` are
written in Brood (`std/prelude/control.blsp`) — see [primitives.md](primitives.md).

**`:trace` is the call stack at the raise** — a list of frames, innermost first,
each a `{:fn <name> [:file <file> :line <l> :col <c>]}` map whose location is the
**call site that entered the frame** (absent fields are omitted; anonymous frames
have no `:fn`). It covers the frames between the raise and the `catch` that
caught it, capped at 32 (deep recursion keeps the innermost frames — the end
that shows the cycle). Proper tail calls collapse into their caller's frame, so
a tail chain `outer → middle → boom` shows one frame named for where the chain
ended — a direct picture of the real (O(1)-stack)
frame structure. Uncaught errors print it as `at fn (file:line:col)` lines under
the diagnostic; it costs nothing on the non-throwing path. A frame that was running
as **JIT-compiled native code** when the error passed through it is recorded too
(KI-117), with its `:fn` and `:file` but no `:line`/`:col` — the call site lives in
the caller's instruction stream, which native code does not keep — so a trace
through hot code has the same frames as through cold code, less those positions.
A user `(throw v)` carries `v` verbatim (no map, so no `:trace` on the caught value).
The position and trace still exist — the raise collected them as it unwound — and
**`(%try-described thunk handler)`** is how a handler reads them for any thrown value:
it calls `(handler caught error)`, where `caught` is exactly what `catch` would bind and
`error` the raise's description, `{:kind :message [:code :file :line :col :hint :trace]}`
(for an error map the language raised, the two are the same map). Tooling that reports a failure it did
not raise — `nest mcp` answering a tool handler's `throw` — uses it:

```clojure
(%try-described (fn () (throw "bad input"))
  (fn (caught error) [caught (get error :line)]))   ;~> ["bad input" 1]
```

Because a thrown value can be anything, **`(error-message e)`** is the
shape-agnostic accessor: a raised string as-is, the `:message` of an error map,
else the value's printed form. A `catch` handler that just wants a human string
uses it directly instead of branching on `string?`/`map?`:

```clojure fragment
(try (risky) (catch e (log (str "failed: " (error-message e)))))
```

Type errors are **self-identifying**: they name the operation, the type it
wanted, and the tag + printed form of what actually arrived — e.g.
`type error: +: expected number, got string ("x")`. The tag word is the
[`type-of`](#predicates) name, so an error and `type-of` always agree.

That is a promise the *implementation* has to keep, and `get`/`nth` didn't until
2026-07-26 (ADR-164): a keyword key on a vector or list fell through to `nth`'s
integer arithmetic and surfaced as `-: expected number, got keyword` — naming a
helper the caller never wrote — and the same key on a **string** silently returned
`nil`. Both now name `get`/`nth` and say what was expected. If you find a diagnostic
that names an internal, treat it as a bug of the same class.

## Dynamic variables

A **dynamic variable** is a global whose value can be temporarily overridden for
the *dynamic extent* of a body — the call tree it encloses — and then restored.
It's the Lisp "special variable", for config-style knobs (a print depth, a
current output sink) that a deep callee should read without threading the value
through every intermediate call.

```lisp
(defdyn *indent* 0)              ; declare a dynamic var with a default

(defn level () *indent*)         ; reads *indent* — whatever is bound right now

(level)                          ;=> 0   ; *indent* is its default
(binding (*indent* 4) (level))   ;=> 4   ; rebound for this dynamic extent
(level)                          ;=> 0   ; restored afterwards
```

- **`(defdyn *name* default)`** declares `*name*` dynamic and gives it a default.
  The earmuffs (`*…*`) are convention, not syntax. Reading the var anywhere
  yields the default until a `binding` overrides it. The declaration also makes the
  name **ambient** — never namespaced, so a `def` of it from *any* module rebinds
  this one root binding (see [Namespaces](#namespaces)). Declare it before the
  first use in a file.
- **`(binding (*a* va *b* vb …) body…)`** evaluates the value expressions, binds
  each dynamic var for the duration of `body`, and **restores the previous values
  on exit — even if the body throws**. Bindings nest; the innermost wins. A
  reference resolves *dynamically*, at the moment it's evaluated, against the
  caller's bindings — not lexically where the function was defined.
- **`(reflect/dynamic? x)`** is true when `x` is a symbol declared with `defdyn`.

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

**The declaration is per-runtime.** Which names are dynamic is a fact of the
runtime, shared by all of its processes — a `defdyn` in one process is visible to
its siblings — but not by separate runtimes (nodes, or several embedded
interpreters in one OS process), each of which starts from the prelude's marks.
`%isolate` rolls back a mark made inside its thunk along with the binding.

`defdyn`/`binding` are Brood macros over a tiny kernel (`%declare-dynamic`,
`%binding`, `reflect/dynamic?`) — no new special form, the `try`/`catch` precedent.

## Output ports and logging

`io/write`/`io/puts` don't write to stdout directly — they write to the **current
output port**. A *port* is just a one-argument function `(fn (s) …)` that consumes
a ready string; the dynamic variables `*out*` and `*err*` hold the current
stdout/stderr ports. The defaults write to the real streams (and `*out*` honours
the `with-out-str` capture), so out of the box `io/puts` behaves exactly as you'd
expect. The point is that you can **redirect** it.

`std/io.blsp` gives the port toolkit — constructors and the `with-out`/`with-err`
scoping macros (thin wrappers over `binding`). Pull it in with `(:use io)` so the
names read bare — otherwise a qualified `io/process-port` reference auto-loads the
module but leaves the names qualified:

```lisp fragment
(defmodule my-app (:use io))

(with-out (fn (s) (collect s))             ; a bare 1-arg fn is a port: route output to a callback
  (io/puts "captured by collect"))

(with-out (process-port editor)            ; route output to another process …
  (io/puts "sent as [:io-write \"…\\n\"]"))
```

A **`process-port`** sends each string to a process as `[:io-write s]`. That is
how output reaches a *buffer*: the process that owns the buffer (an editor's
`*Messages*`, say) receives the message and appends it. The string crosses the
process boundary as a copied message — async and share-nothing, never a mutated
value — which is exactly why it's safe. (Dynamic bindings don't
reach a `spawn`ed child, so a child starts with the default `*out*`; hand it a
port explicitly if it should redirect too.)

A port is any value implementing the **`Port`** ability, whose one op is `emit` — so
a bare 1-arg fn is a port (that is the `:fn` impl), and so is a port *record* that
carries its target and prints as itself (`#<port stdout>`, `#<port file /tmp/app.log>`).
`io/port?` tests it, and your own type joins with `defrecord` + `impl Port`. `*out*`/`*err*`
still hold a plain fn, which the writers call directly — so printing pays no
dispatch cost, `io/write` gains no special cases, and `with-out-str` is unaffected;
`with-out`/`with-err` adapt a record port at that boundary for you (`port->fn`).

### Logging

`std/log.blsp` is an **async, safe logger** built on the same idea. A logger is
one long-lived process (a `gen` server) holding a list of *backends*; each
log call is a fire-and-forget cast, so it never blocks the caller, and the single
process serialises every write — lines never interleave, and a backend that throws
takes down only that line, not the caller.

```lisp fragment
(defmodule my-app (:use log))

(start-logger)                          ; default: stdout, :info and up
(info "server up" {:port 8080})         ; structured fields are optional
(log :warn "disk low")
;; => [INFO 1736…] server up
;;    [WARN 1736…] disk low
```

Levels are `:debug` < `:info` < `:warn` < `:error`. A **backend** is any value
implementing the **`LogBackend`** ability (one op, `backend-emit`, handed each record).
The stock one is an `io` port + a minimum level + a filter + a formatter, so the logger
*reuses* ports rather than inventing its own sink; build it with `stdout-backend` /
`stderr-backend` / `file-backend` / `fn-backend` / `process-backend`, and add it live:

```lisp fragment
(add-backend (file-backend "app.log"))         ; also append to a file
(add-backend (process-backend buffer-pid))     ; …and to a buffer-owning process
```

`process-backend` is the **log-to-a-buffer** path: the formatted line is sent to
`buffer-pid` as `[:io-write s]` — the same envelope `process-port` uses — so an
editor process can fold it into its `*Messages*` buffer. The default logger is
registered under the name `:logger` (found via `proc/whereis`); `(log …)` falls back to
stderr when none is running, so a log is never silently lost.

For a backend that does something other than write one formatted line — batch records,
emit JSON lines, sample — `defrecord` your own and `impl LogBackend` for it; the logger
takes it unchanged, and `backend-passes?` gives you the same `:min-level`/`:filter` gate
every stock backend honours.

Both `io` and `log` are written in Brood over the process primitives — Rust only
supplies the render/write split behind the writers (`%render`, `%write-out`,
`%write-err`). See `std/io.blsp` and `std/log.blsp`.

## Type annotations

Types in Brood are **optional and advisory** — you never have to write one, and a
program with no annotations checks and runs exactly as before (see
[types.md](types.md) for the set-theoretic model). Two opt-in declaration forms
let you inform — and optionally *enforce* — the type system. Both are macros, not
special forms.

**Advisory in the image, a gate where you want one.** The checker never stops code from
loading into a live image — hot reload depends on that — but its verdict is an exit status.
`nest check` exits non-zero on any gating finding, and `--strict` adds the findings for a
value merely wider than a parameter accepts. A project that wants a type gate
runs `nest check --strict` in CI, as Brood does for `std/` and `tests/` and bedit does for
itself; `(check-allow :category form…)` records a deliberate exception where it is made.

Every finding has a **category**, printed after it (`… [type-mismatch]`) and listed by
`(reflect/check-categories)`. Four are advisory by default — printed, never failing the
check: `:non-tail-recursion`, `:gave-up` (a checker budget ran out), `:deprecated` and
`:beta`; the rest gate (ADR-698). A project moves categories in `project.blsp`, and a module
silences one for its whole file in its header:

```clojure fragment
(project :name "app" :version "0.1.0"
  :check {:advisory [:unused-binding]        ; printed, never gating
          :gating [:non-tail-recursion]      ; fails the check
          :allow [:list-truthiness]})        ; not reported at all

(defmodule app/render "Drawing."
  (:check-allow :non-tail-recursion))        ; this file only
```

`(sig name (params… -> ret))` declares a function's signature. It is a pure
declaration — a runtime no-op — read by the advisory checker, which then flags a
provably wrong call against it (both the argument and the result type flow):

```clojure fragment
(defn area (r) (* 3.14159 r r))
(sig area (number -> number))

(area "circle")           ; warning: area: argument 1 expects number, got string
(string/upper (area 2))   ; warning: string/upper: argument 1 expects string, got number
```

**Either side of the definition** — see "Placement" below. A `sig` is a declaration
wherever it stands; enforcement (when armed) attaches to the binding, not to the form.

**Typed parameters** (ADR-420) say the same thing where the parameters are — the spelling
std uses for a single-clause function, with `(sig …)` kept for what they cannot express (a
multi-arity or pattern-dispatched function, an overload, a value, property keywords):

```clojure
(defn area (w:number h:number -> number)
  "The area of a w×h rectangle."
  (* w h))

(defn greet (name:string &optional (greeting:string "hello") -> string) …)
(defn total (xs: (list int) -> int) …)    ; `name:` then a type FORM
(defn pair (x:int y) [x y])               ; untyped `y`, no `->`
```

A parameter `name:type` binds `name` and types it; `name:` types it with the form that
follows; `-> TYPE` ends the list with the result type. The definition registers exactly the
`(sig …)` its parameters spell — `(sig area (number number -> number))` — so the checker and
the contracts treat the two spellings as one. A position left untyped is the placeholder
`_`: an untyped parameter is unconstrained, and with no `-> TYPE` the result keeps the type
the checker **infers** from the body, so typing one parameter never costs the return
(`pair` above declares `(int _ -> _)`). `_` may be written in a `sig` too. Typed parameters
work on `defn`, `defn-`, `fn` and `defmacro`; a multi-clause `fn`, a destructuring pattern and
a `let` binding refuse a type with an error — declare those with `(sig f (and …))` or on the
enclosing function.

The type grammar: base names — `int float number decimal ratio string symbol keyword
bool nil pair vector list map set bytes failure record fn rope pid ref table socket
subprocess`, plus `any` (everything) and `never` (nothing); the spellings match what
`type-of` returns, with `number` = int∪float, `list` = nil∪pair, and `fn` = closure∪native.
Four more name unions: `seqable` (what the sequence functions walk: a list, vector, set, map
or `bytes`, never a string), `countable`, `numeric` and `ordered`.
Then function arrows `(p… -> r)`, element-typed
sequences `(list E)` / `(vector E)`, unions `(or A B …)`, intersections
`(and A B …)` and the complement `(not T)` — so "anything but nil" is `(and any
(not nil))` — literal (singleton) types — a bare keyword/int/bool/string
(`:foo`/`5`/`true`/`"GET"`) — any combination composing freely in one `(or …)`
(see [type-int-literals.md](type-int-literals.md) and
[type-bool-string-literals.md](type-bool-string-literals.md)), type
variables (`?A`), key/value-typed maps `(map K V)` (see
[type-map-kv.md](type-map-kv.md)), and heterogeneous record shapes `(record
:k1 T1 :k2 T2 …)` with required-by-default fields and an `(optional T)` wrapper
for optional ones (see [type-records.md](type-records.md)). A record is **closed** —
it names every key the value has, and a key it doesn't declare reads as `nil`; write
`(record &open :k T)` for the permissive form. That is what makes a tagged union
usable: `(get r :ok)` over `(or (record :ok int) (record :error string))` is
`int | nil`, because the second alternative says `:ok` is absent. An int can carry an
interval, `(int 0 _)` (`_` is an open end), and any countable a length, `(len (vector int)
3 3)` — what `count` reads and a positional read is proven present by (see
[type-intervals.md](type-intervals.md)); `(list E)` is a list that may be empty, and the
non-empty one is `(len (list E) 1 _)`.

**Properties.** Keywords after the type — or alone — declare what the checker holds the
function to (see [type-properties.md](type-properties.md)): `:pure` (no effect: no
message, no I/O, no table, no clock or randomness, through every function it calls) and
`:total` (every self-call decreases an argument, and no `match` in the body can fall
through). `(sig area (number -> number) :pure)`; `(sig walk :total)`. A `ui-memo` thunk is
held to `:pure` without being asked.

A type-expression the checker cannot read is **reported**, not guessed: a
misspelled name (`strng`) or constructor (`(tupel int)`), a declaration whose
arity contradicts the definition it annotates, and a declaration for a name the
file never defines are all warnings. (An unknown *capitalised* name is taken to
be an ability used as a type, whose module this check may not have loaded, and
stays silent.) A union of two different structured types keeps both — `(or
(tuple int) (tuple string))` rejects `[true]` and admits either shape.

A `match` whose scrutinee's declared type is a *pure* enumerable literal type
(any combination of the literal kinds above, plus `nil`) gets two more checks
for free: **exhaustiveness** — a missing arm is flagged unless a catch-all
clause is present (see
[type-match-exhaustiveness.md](type-match-exhaustiveness.md)) — and
**redundancy** — a clause whose literal duplicates one already handled earlier
is flagged as unreachable dead code (this one is purely structural, so it
fires on any hand-written same-symbol `%eq`-literal `if`-chain too, not just
`match`-generated ones; see
[type-match-redundancy.md](type-match-redundancy.md)).

`(sig! name (params… -> ret))` declares the **same** signature *and enforces it at
run time*, whatever the mode: each argument and the result are checked on every call,
and a mismatch throws a **blamed** contract error (an opt-in "strong arrow"). The error
is a map — `{:kind :contract :blame :caller :function 'area :argument 1 :expected number
:got "circle" :message …}` for an argument (the caller's fault), `:blame :callee` for a
result (the callee's) — so `(get e :blame)` names the party and `error-message` reads it.
Caught, it also has `:code "E0080"`, `:file`/`:line`/`:col` and `:trace`, like a built-in
error.

```clojure
(defn area (r) (* 3.14159 r r))
(sig! area (number -> number))
(area "circle")   ;=> RAISED area: argument 1 expected number, got :string ("circle")
```

**Enforcement is a property of the binding, not of the declaring form** (ADR-381). The
kernel offers a name's binding to the contract policy when the binding comes to exist — at
its `def`, at a `sig`/`sig!` landing on an already-defined name, and when a std module's
bindings arrive (from source or from the stdlib image) — and binds a checking shim in its
place when the declaration is armed. So placement is free (above the definition, where
documentation goes, or below), a hot reload re-wraps the new definition, and a shim keeps
the function's name, docstring, privacy, `meta` and — for a fixed-arity definition — its
arity. An `&optional`/`&` definition gets a variadic shim that checks the declared prefix
and passes the rest through, so the callee's own defaults still apply.

**Dev mode arms every `sig`.** `nest run` and `nest test` set `BROOD_CONTRACTS=1`, so
every declaration in the project and in std is enforced while you develop and test;
`BROOD_CONTRACTS=0` opts a run out, and a released bundle never arms. The prelude's own
sixteen declarations (`nth`, `conj`, `assoc`, …) are enforced only under
`BROOD_CONTRACTS=all` — they are the hottest names in the language, each a thin wrapper
over a native that already raises the precise error. `sig` alone is checker-only (zero
runtime cost) in a release; `sig!` adds the runtime guarantee everywhere.

**The prelude carries signatures too**, ~16 of them, enforced per runtime at boot under
`all` (the shared prelude is frozen; a shim is an ordinary runtime rebinding).

Adoption is broad — **over 800 declarations, `sig`s and typed parameters, across more than
90 std modules** (ADR-153 started it in
`std/path`, `std/json` and `std/set`). The checker enforces them at every call site, in
any module, and result types flow (a `bool` result handed to `string/upper` is caught).
The checker treats `sig` and `sig!` identically. Writing a *type* never changes
behaviour; opting into *enforcement* does. Full design:
[type-annotations.md](type-annotations.md) (ADR-082).

### Advisory lints (non-type warnings)

`nest check` / `brood --check` emit several additional warnings beyond type
misuse, with zero false positives. Like a type warning they never stop code loading, and
their categories (`:unused-binding`, `:unused-import`, `:unused-private`) gate `nest check`
unless the project's `:check` key moves them:

**Unused bindings** — a name a binding form introduces that never appears in its
visible scope. Every binding form is covered, with the same pattern grammar each uses:
`let`/`letrec` (including destructuring), `with`, `if-let`/`when-let`/`if-some`/
`when-some`, `as->`, `match` and `receive` clauses, `catch`, the loop forms
(`doseq`, `for`, `fold-for`) and anonymous `fn` parameters in
either spelling. A definition's own parameters (`defn`, `(def f (fn …))`) are its
published arglist and are not reported. Exempt: names prefixed with `_` (intentional
don't-care), a name that shadows a global, and a name the pattern itself reads (a
repeat `[x x]`, or a `bytes` size naming an earlier segment). A pin `^x` and a
quoted `'sym` bind nothing.

```clojure fragment
(let (x 1 y 2) x)                  ; warning: unused let binding: y
(let ([c r] (term/size)) c)        ; warning: unused let binding: r
(match msg ([:attach c rows] c))   ; warning: unused pattern binding: rows
(try (f) (catch e nil))            ; warning: unused catch binding: e
(fold-for (acc 0 x xs) x)          ; warning: unused loop binding: acc (it never folds)
(map xs (fn (acc x) acc))          ; warning: unused parameter: x
(let (_y 1) 2)                     ; silent — _-prefix means intentional
```

**Unused `:use` imports** — a `(:use mod)` clause whose contributed public names
are never referenced in the file. Only fires when the module contributed at least
one name (so a failed load or an empty module is silent).

```clojure
(defmodule my/app (:use io) (:use json))
(defn handler (x) (encode x))
; warning: unused :use import: io  [unused-import]
```

**Unused module-private defns** — a `defn-`/`def-` private that is never referenced
anywhere in the project — neither as a same-module unqualified call nor a
cross-module / test `mod/name` reference. Checked at the *whole-project* layer
(`nest check`), not by a single-file check: a private is legitimately reached from a
test by its qualified name (with `(:use-internals …)`), which a per-file scan can't
see. Public names are never checked.

```clojure
(defmodule my/mod)
(defn- parse-helper (s) …)   ; warning: unused private function: parse-helper
(defn run (s) s)             ; (parse-helper is defined but never called)
```

All three lints share the "zero false positives" contract: they are conservative
(count any occurrence, including in binder positions, as "used") and emit nothing
when static information is ambiguous.

## Processes (concurrency)

**Green processes**: cheap, lightweight, share-nothing (each runs
with its **own data heap**), communicating only by **message passing**. They run
on a small pool of worker threads (≈ one per core, or the CLI's `-j N`), so they
use every core; scheduling is **preemptively fair** — a CPU-bound process yields
its worker after a reduction budget, so one busy loop can't freeze the runtime.
That holds wherever the code runs: inside `try` and `binding` bodies (both run in the
bytecode driver's own loop, ADR-400/402), inside native (JIT) code at any call depth —
a preempt unwinds native frames into interpreter frames (ADR-401) — and between the
elements of a `reduce`/`fold`/`map` over a vector or range (ADR-403). What cannot yield
mid-way — a single long native builtin, a pure numeric recursion running in the unboxed
register worker — yields as soon as it returns, and the system monitor's
`:long-schedule` event reports it.
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
child's own pid** — to hand the parent's pid in, bind it in an enclosing `let` first.

| Form | Meaning |
|---|---|
| `(spawn expr)` | Run `expr` (unevaluated) in a new green process; returns its pid. Free locals are captured — copied into the child's own heap as a message would copy them, while the code itself stays shared (ADR-405) — and `(self)` inside is the *child's* pid. |
| `(spawn-link expr)` | As `spawn`, but the child is **linked to the caller atomically** — before it can run, so even an instant exit reports its *true* reason. What supervised children use. See *Links* below. |
| `(spawn-monitor expr)` | As `spawn`, but the caller **monitors** the child atomically — returns `[pid ref]`, and the `[:down …]` carries the *true* reason even for an instant exit. One-shot, never kills. See *Links* below. |
| `(send target msg)` | Copy `msg` into `target`'s mailbox (non-blocking; a dead/unknown target is a no-op). `target` is a pid (local **or remote** — see [Distributed nodes](#distributed-nodes)) or a `{:name :node}` address. |
| `(receive clause...)` | Take the first matching message (see below); suspend until one arrives. `(receive)` with no clauses takes the next message. |
| `(self)` | Your own pid — a `:pid` value carrying this node's identity. |
| `(ref)` | A fresh unique reference token — see *Synchronous calls* below. |
| `(monitor pid)` | Watch `pid`; returns a monitor `ref`. See *Monitors* below. |
| `(demonitor mref)` | Drop the monitor created by `(monitor …)`. |
| `(exit pid reason)` | Send an exit signal to a local process. `reason` `:kill` is the **untrappable** hard kill — the target dies at its next reduction tick, or immediately if parked, even in a tight loop. Any other `reason` is the **soft** signal — the target dies at its next `receive` (a tight non-`receive` loop won't honour it). Monitors fire `[:down ref pid reason]`. No-op for a dead/unknown pid. |
| `(dev/spawn-count)` | How many green processes have been spawned since the program started. |
| `(dev/peak-threads)` | High-water mark of processes running *simultaneously* (bounded by the worker pool). |
| `(dev/worker-threads)` | Size of the worker-thread pool (≈ `nproc`, or `-j N`). |

### Selective receive

`receive` takes **pattern clauses** — the same grammar as `match`/`fn`
([Pattern matching](#pattern-matching)). It scans the mailbox in order, runs the
**first message that matches any clause**, and leaves non-matching messages
queued for a later `receive` (true selective receive — no head-of-line
blocking). Clauses may carry a `:when` guard.

```clojure fragment
(receive
  ([:say text]      (io/puts text))     ; clause = (pattern [:when guard] body...)
  ([:add a b]       (+ a b))
  (n :when (int? n) (handle-int n)))
```

An optional trailing **`(after ms body...)`** clause bounds the wait: if no
message matches within `ms` milliseconds, `body` runs instead. `(after 0 …)` is a
non-blocking poll. A receive with ONLY an `after` clause, `(receive (after ms body))`,
matches nothing: it waits `ms` and leaves the mailbox alone, like `proc/sleep`
(ADR-520). `(receive)` with no clauses at all takes the next message. Because the
timeout body is ordinary code, a timeout is **catchable** — throw from it and catch with `try`/`catch`:

```clojure
(try
  (receive ([:pong] :ok)
           (after 5000 (throw [:timeout])))   ; raise a structured, catchable value
  (catch e e))                                 ;=> [:timeout]   ; on timeout
```

Messages are **copied** between processes. You can send a **closure** too: it
travels as data — its body is S-expression forms, its captured locals are copied,
and its free globals re-resolve on the receiver (so it runs on any node that has
the same definitions). This is what makes `(spawn expr)` shippable to another node.
A *builtin* can't be sent (it's a Rust function with no portable form) — reference
it by the symbol naming it instead, since code is shared. `receive` is a macro
over the `%receive` primitive, built on the `match` compiler — no new special
form. The primitive only *selects* a clause (answering which one matched and what
its pattern bound); every clause **body is emitted at the call site**, so bodies
compile into the enclosing function's own code and a receive loop's tail call is
an ordinary tail call in that function (ADR-155). See
[concurrency.md](concurrency.md) and [scheduler.md](scheduler.md) for the model,
and [pattern-matching.md](pattern-matching.md) for the clause grammar.

### Synchronous calls (and why there's no `await`)

`send` is fire-and-forget. To wait for a result, you don't need an `await`
primitive — the **blocking `receive` is the synchronisation**. The idiom is
the cast/call distinction: a *cast* is a bare `send`; a *call* is a
request whose reply you `receive`. The catch with concurrent calls is telling
replies apart, which is what **`(ref)`** is for: a fresh, opaque, unforgeable
token you put in the request and the server echoes in the reply, so a pinned
`^ref` in your `receive` matches only *your* answer (other replies stay queued).

```clojure
(defn reply (to tag v) (send to [:reply tag v]))
(defn call (pid req)
  (let (tag (ref))                       ; a unique token for this call
    (send pid [:call (self) tag req])
    (receive ([:reply ^tag v] v))))      ; block for exactly this reply
```

A script exits when its *main* process returns, so ending on a `call` (which
ends on a `receive`) is how you ensure spawned work finished before exit — no
separate `await`/join. `(ref)` values are their own type (`ref?`, `:ref`),
compared by identity, and may be sent in messages. (`call`/`reply` aren't in the
prelude yet — see `examples/life.blsp`.)

#### One-shot reply aliases (`%ref-deactivate`)

A call with a deadline has a loose end: if you give up and the server answers
anyway, its `[:reply ^tag v]` lands in your mailbox carrying a token **no receive
of yours will ever match again**. It is unreachable garbage — it never gets
consumed, it grows the mailbox without bound over a retry loop, and every later
*selective* receive re-scans it.

Having the caller simply **exit** on a call timeout makes a late reply moot, but
costs the caller. Brood instead gives the reply a **one-shot alias**: once the alias
is deactivated the runtime *drops* later replies addressed to it. It needs no new
value kind, because a `ref` is already the unforgeable per-request token a reply
carries:

```clojure
(def r (ref))
(%ref-deactivate r)   ;; r is a ref THIS process minted
```

From then on, a message **addressed to `r`** — a keyword-tagged vector whose second
element is `r`, i.e. the `[:reply r v]` / `[:down r pid reason]` shape — is dropped
**at delivery**. It never enters the queue, so nothing has to scan past it later.
Only the minting process can deactivate its own refs (the set lives on its own
mailbox), already-queued messages are untouched (like `demonitor` without `flush`),
and an alias is one-shot: the entry is forgotten as soon as it swallows a message.
The per-mailbox set is a fixed eight entries, so it cannot grow; past that the
oldest is evicted, which merely restores the old behaviour for one long-abandoned
ref rather than dropping anything wrong.

`gen/call-timeout` uses it on both give-up paths (deadline and server death), which
is why a timed-out `gen/call` leaves an empty mailbox behind.

The opt-in **`task` module** (`(:use task)`) packages the common "run this
thunk off my loop, with a timeout, cancellable" pattern over `spawn`/`receive`/
`exit`: `(task/task thunk opts)` returns a handle and delivers a tagged `[:task-done
handle v]` / `[:task-error handle msg]` / `[:task-timeout handle]` message to
`:reply-to`; `task/cancel` stops it early; and `(task/await thunk timeout-ms)` is the
*synchronous* run-with-timeout that blocks for the value (throwing on error or
timeout). This `await` is a userland convenience for bounding a single
computation — distinct from the `gen/call` idiom above, which is the
right tool for request/reply to a long-lived process.

### The `gen` server framework

`std/proc/gen.blsp` packages the request/reply idiom above into a
server framework — ~500 lines of Brood (`defstatem` and `{:via …}` addresses included) over `spawn`/`send`/`receive`/
`ref`/`monitor`, no kernel surface (ADR-099). A server carries one immutable
state value through a tail-recursive `receive` loop; `defserver` declares how it
handles each kind of message.

`gen` is an **ordinary module** with a bare namespace, so its client API is
`gen/start`, `gen/call`, `gen/cast`, `gen/stop`, `gen/defserver` — or bare
after `(:use gen)`. (It was briefly prelude-bundled with those names bare; that
seized `call`, `cast` and `stop` as un-redefinable global names, which a framework
does not get to own. `(def call …)` is yours again.)

```clojure
(defmodule my-app (:use gen))   ;; refers defserver / start / call / cast / stop bare

(defserver counter (n)
  (init  (do (io/puts "up") n))            ; runs once at startup; returns the initial state
  (cast  :inc            (+ n 1))          ; fire-and-forget; body = next state
  (call  :value          [n n])            ; synchronous; body = [reply next-state]
  (query :double         (* n 2))          ; synchronous read-only; body = the reply, state unchanged
  (info  [:down _ p r]   (do (log p r) n)) ; a non-envelope message (monitor/link/timer/raw send)
  (terminate reason (io/puts "down: " reason)))  ; runs on (stop); body for cleanup

(def c (start counter 0))
(cast c :inc)              ; fire-and-forget
(call c :value)            ;=> 1  ; synchronous, 5 s default timeout
(stop c)                   ; graceful shutdown — runs terminate, then ends the loop
```

Without `(:use gen)` the same three lines are `(gen/cast c :inc)`,
`(gen/call c :value)`, `(gen/stop c)`, and the definition head is
`(gen/defserver counter (n) …)`.

There are three message clause kinds plus two lifecycle hooks: **`cast`** (body → next state), **`call`** (body →
`[reply next-state]`; the caller blocks for the reply), **`query`** (a read-only
`call` — body → reply, state untouched), and **`info`** — a message that is *not*
a cast/call envelope: a monitor `[:down …]`, a link `[:EXIT …]`, a timer tick, or
a plain `send`. Optional **`init`** runs once at startup (the place to
`(proc/trap-exit true)`, `(monitor …)`, arm a timer, or transform the seed) and
**`terminate`** runs on a clean `(stop pid)`. Envelope clauses are always matched
before `info` clauses, and **any message matched by no clause is dropped** rather
than left to pile up in the mailbox.

Client API: `(gen/cast pid payload)` casts; `(gen/call pid payload)` calls and
blocks up to 5 s (it `monitor`s the server, so a *dead* server raises at once
instead of hanging); `(gen/call-timeout pid payload ms)` sets a custom deadline;
`(gen/stop pid)` ends the loop; `(gen/code-change pid)` asks a running server to
migrate its state after a hot reload. Spawn with `(gen/start f state)`, which takes an
options map: `{:link true}` links the server to the caller and
`{:name :k}` registers it for `proc/whereis`, together or apart (ADR-433). A `defserver` server
composes directly under `supervisor` (see `std/proc/supervisor.blsp`).

**A call that times out leaves nothing behind.** `gen/call-timeout` mints a fresh
`(ref)` for the reply and, on the deadline path, **deactivates** it with
`(%ref-deactivate r)` — the kernel then *drops a later reply carrying that ref at
delivery*, so it never enters the caller's mailbox at all. Without it, a slow-but-live
server answering after the deadline would leave an unmatchable `[:reply ^r v]` queued forever,
growing the mailbox over a retry loop and re-scanned by every later selective
receive. See "One-shot reply aliases" under Synchronous calls above.

**State machines: `defstatem`** (ADR-437). A server whose behaviour depends on a
named state lists one block per state; `gen/start`, `gen/cast`, `gen/call` and
`gen/stop` drive it unchanged:

```clojure fragment
(defstatem door (code)
  (:locked
    (cast [:digit d] :when (= d code) [:next :open code {:timeout 5000}])
    (call :state (gen/reply reply :locked) [:keep code]))
  (:open
    (timeout [:next :locked code])
    (cast [:digit _] [:postpone])
    (call :state (gen/reply reply :open) [:keep code])))
```

A block's clauses are `(cast PATTERN body…)`, `(call PATTERN body…)` (binds
`reply`, answered with `gen/reply`, so a call can be answered later or
postponed), `(info PATTERN body…)` and `(timeout body…)`; a pattern may take a
`:when` guard. Each body returns an **action**: `[:next state data]` moves (and
`[:next state data {:timeout ms}]` runs the new state's `timeout` clause if the
machine is still there after `ms`; every `:next` cancels the pending timeout);
`[:keep data]` stays (a pending timeout stays armed); `[:postpone]` keeps the
event until the state changes, then handles it again, oldest first; `[:stop
data]` ends the machine. An `(any clause…)` block applies in every state after
the state's own clauses, and an event nothing matches is dropped. The machine
starts in the first state listed, or in what `(init body…)` returns (`[state
data]`, optionally with opts); `(terminate reason body…)` runs on `gen/stop` and
on `[:stop data]`.

### A process registry: `names`

`proc/register` binds a keyword to one pid. A **registry** (`std/proc/names.blsp`,
ADR-445) binds a key that is any value — `[:session 42]` — to one holder (`:unique`,
the default) or to many (`:duplicate`), each with a value beside it:

```clojure
(names/start :sessions)                              ; a registry, registered as :sessions
(names/register :sessions [:user 7] (self) {:since 0})
(names/whereis :sessions [:user 7])                  ; the pid, or nil
(names/lookup :sessions [:user 7])                   ; [[pid {:since 0}]]
(names/start :topics {:keys :duplicate})             ; a key holds any number of processes
```

The registry is a server that owns a table, so writes are serialised and a `:unique`
key another live process holds is refused with a `failure` naming it. Reads —
`lookup`, `whereis`, `count` — go straight to the table, in the caller. A holder's
entries go when it dies, and every entry goes when the registry does.

A `{:via registry :key key}` address works wherever `gen` takes a pid: `gen/call`,
`gen/cast`, `gen/stop`, `gen/code-change`, and `gen/start`'s `:name`, which registers
the new server (a taken key stops it and answers the failure). A call to a key nobody
holds raises; a cast to one is dropped.

```clojure fragment
(gen/start counter 0 {:name {:via :sessions :key [:counter 1]}})
(gen/call {:via :sessions :key [:counter 1]} :value)
```

**Process groups** (`std/proc/groups.blsp`, ADR-446) span the cluster. Each node runs one
group server (`groups/start`, registered as `:groups`); a process `join`s a group by any
name, and `members` lists the group's processes on every connected node:

```clojure
(groups/start)
(groups/join :workers)                         ; the caller; or (groups/join :workers pid)
(groups/members :workers)                      ; every member, on any node
(groups/local-members :workers)                ; this node's only
```

Each server is the authority for its own node's members — it monitors them and tells its
peers of every join, leave and death — and a new peer gets a full sync, so every node holds
its own copy and reads it without a message. Membership is eventually consistent: a join is
seen elsewhere once its message arrives, and a node's members go when its link is seen to
drop. There is no cluster-wide lock or unique name.

### Monitors

`(monitor pid)` starts watching another process and returns a monitor `ref`.
When that process dies, the watcher receives one message:

```clojure fragment
[:down <monitor-ref> <pid> <reason>]
```

`reason` is `:normal` for a clean return, `[:error <error-map>]` for a crash —
the map a `catch` in the dying process would have bound: a thrown map as thrown, a
contract violation with its `:kind :contract`, `:blame`, `:expected` and `:got`, a
built-in error as its structured `{:kind :message [:code :file :line :col :hint
:trace]}` map (call `:trace` included). Any of
those standard fields the bound map lacks is filled in from the error, and a throw
of a non-map reads as `{:kind :user :message <printed value> …}`, so the reason is
always a map, and a supervisor can log `(get m :message)` and walk `(get m :trace)`
from the reason alone — and
`:noproc` if `pid` was *already* dead when you called `monitor` (the DOWN is then
delivered immediately). The monitor is **unidirectional** (it never affects the
watched process) and **one-shot** (it fires once). `(demonitor mref)` drops it,
best-effort — a DOWN already queued is not recalled. Pin the ref to wait for a
specific process's death and ignore unrelated messages:

```clojure fragment
(def w (spawn worker))
(def m (monitor w))
(receive
  ([:down ^m _ :normal] :finished)
  ([:down ^m _ reason]   (restart reason)))   ; supervision, in-language
```

Monitors are the one kernel mechanism a **supervisor** is built from: watch your
children, and on a non-`:normal` DOWN, restart per a strategy — all expressible
in Brood.

### Links

`(link pid)` ties the current process and `pid` together **symmetrically**
(`(unlink pid)` unties, `spawn-link` spawns pre-linked). When
either side dies abnormally, the other is notified: a process that set
`(proc/trap-exit true)` receives a trappable `[:EXIT pid reason]` message; a
non-trapping process **dies too** — propagation, cascading through *its* links
in turn. A `:normal` exit never kills a non-trapping peer. The propagated death
carries the **originating reason**: if `a` crashes with `[:error {…}]`, a linked
non-trapping `b` dies with that same reason (and so does `c` linked to `b`), so
monitors anywhere in the fallen tree report the root cause — not a blanket
`:kill`. Links are what `supervisor`'s trapping supervisor loop is built
on; remote (cross-node) links deliver the same shapes, plus `:noconnection` on
a net-split.

The program a `brood file.blsp` / `nest run` runs is itself a process, and an exit
signal kills it like any other — sleeping, receiving or computing. An abnormal
reason ends the program at once with exit status 1 and
`runtime error: killed by an exit signal: <reason>`; a `:normal` one (`(exit root
:normal)`) ends it with status 0. Trapping exits turns link signals into
`[:EXIT pid reason]` messages here too.

#### `spawn-link` — spawn and link atomically

`(spawn-link expr)` spawns `expr` in a new process **already linked** to the
caller and returns its pid. It takes one expression, like
`spawn`, and a `(fn () …)` body is passed through rather than re-wrapped; there
is no named form (`proc/register` the child if it needs a name).

The point is the **atomicity**, not the brevity. The link is registered while
the child's mailbox is live but *before* the child is enqueued to run, so the
child cannot exit before the link exists — its eventual death always reaches you
as `[:EXIT pid reason]` carrying the **real** reason. Hand-rolling it as
`(let (p (spawn expr)) (link p) p)` leaves a gap: a child that finishes inside it
is linked *dead*, and link-to-dead reports `:noproc` — which **replaces** the
true reason, so a clean `:normal` return arrives looking like a failure.

```clojure
(proc/trap-exit true)

(let (p (spawn-link :ok))                  ; child returns immediately
  (receive ([:EXIT ^p reason] reason)))    ;=> :normal   ; always

(let (p (spawn :ok))                       ; the hand-rolled version: `link` races the child
  (receive (after 20 :settle))             ; ...lose the race (here, deliberately)
  (link p)
  (receive ([:EXIT ^p reason] reason)))    ;=> :noproc   ; the :normal is gone
```

That difference is not cosmetic: a supervisor whose restart policy keys off the
reason (`:transient` restarts only on an *abnormal* exit) will spuriously restart
a child that merely finished fast — a real bug this repo shipped and fixed
(2026-06-14). And the gap makes the hand-rolled form untrustworthy rather than
merely sometimes-wrong: drop the `after` above and `link` usually wins, so the
failure only appears under load — exactly where a supervisor matters.

**Use `spawn-link` for anything supervised.** `std/proc/supervisor.blsp`'s
`:start` thunks do (`(fn () (spawn-link (worker …)))`), and `gen`'s
`(gen/start f state {:link true})` is the same guarantee for a `defserver` server. The prelude
macro expands to the `%spawn-link` primitive (ADR-067).

#### `spawn-monitor` — spawn and monitor atomically

`(spawn-monitor expr)` is the same guarantee on the **monitor** side,
returning `[pid ref]`. The child is monitored before it is
enqueued, so `[:down ref pid reason]` always carries the child's real reason.
One-shot and unidirectional like `monitor`: it never kills, and `demonitor`
takes the returned ref.

```clojure
(let ([p r] (spawn-monitor (error "boom")))
  (receive ([:down ^r _ reason] (first reason))
    (after 500 :lost)))                    ;=> :error   ; always
```

The gap it closes is the link one exactly: `(let (p (spawn expr) r (monitor p)) …)`
*looks* right and holds only while the spawner never yields between the two
bindings. Open a gap — a loaded scheduler, or a boundary trace wrapping the code —
and the child exits first, `monitor` takes its already-dead branch, and the DOWN
carries `:noproc` **in place of** the reason. Measured: adjacent, 0 of 300 runs
lost the reason; with one 5 ms yield between them, 40 of 40 did.

Nothing raises when that happens — the receive fires, with a reason that says only
"already gone" — so prefer this form whenever the reason matters. Monitoring a pid
that is *genuinely* already dead still reports `:noproc`, which is correct and
unchanged. The prelude macro expands to the `%spawn-monitor` primitive (ADR-309).

### Crash reports

A process nobody links to or monitors still crashes somewhere: the kernel prints
one line to stderr — `process 7 died: worker.blsp:14:9: runtime error: division by zero` — and
appends it to `.brood_crash_dump`, but that line has no trace, no hint, and prints
again for every iteration of a crash loop. **The default crash reporter**
(`std/proc/crash-report.blsp`, ADR-305) is a `proc/system-monitor` subscriber
selecting `:exit-abnormal`, armed by `brood file.blsp`, `nest run`, a released
bundle and the REPL before the program starts (`nest test` never arms it — the
runner reports its own failures), and it prints one report per **crash site**:

```
[crash] #<pid a/7> exited: division by zero
        kind :runtime  code E0040  at src/worker.blsp:14:9
        hint: guard the denominator: (when (not (= y 0)) (/ x y))
        at worker/step (src/worker.blsp:14:9)
        at worker/loop (src/worker.blsp:9:5)
        (further crashes at this site are not reported)
```

The site is the raise's first positioned frame (`kind file line`), so a worker
crashing thirty times in a supervisor's restart loop costs one report and the
supervisor's own give-up another; a raise with no position keys on
`(kind message)`. Deliberate exits — `:normal`, `:kill`/`:killed`, `:shutdown`,
`[:shutdown x]` — are never reported: a supervisor tearing a subtree down is
not a fault. The process that armed the reporter is excluded too (its crash is
the CLI's to print). A *handled* crash is still reported — a linked, trapping
parent that restarts its child does not make the child's error less worth
reading.

- `(crash-report/start [opts])` arms it (idempotent; registered as
  `:crash-reporter`); `{:sink f}` hands each rendered report to `f` instead of
  stderr — the seam for a host that captures reports (an editor's message
  area) and for tests. `(crash-report/stop)` disarms and returns once the
  subscription is gone; `(crash-report/running?)`; `(crash-report/render pid
  reason)` is the pure formatter. The entry points call the prelude's
  `(%crash-report-arm-default)`, which subscribes without loading the module and hands
  over to it on the first crash (ADR-313) — a no-op under **`BROOD_NO_CRASH_REPORT=1`**.
- The kernel's one-liner yields while any abnormal-exit subscriber is armed
  (see `proc/system-monitor` above), so opting out restores it: nothing is ever
  reported twice, and nothing goes unreported.
- A program that CATCHES its errors (an editor's command, a server's handler) sends them
  somewhere instead: `(crash-report/describe-error error [context])` turns a caught error map,
  an exit reason or any value into a report map — `:kind`, `:code`, `:message` (redacted,
  capped), `:site` and `:frames` with project-relative paths, `:brood`, `:os`, `:context` —
  under a `:signature` that is the same for the same bug on every machine (its kind, site and
  calling functions; no message, value or home path). `(crash-report/redact text)` takes the
  home directory, bearer tokens, `password=`-style values and PEM keys out of text, and
  `(crash-report/post url report)` sends a report as JSON (blocking: call it from a task).

### Applications and shutdown signals

**`(os/trap sig)`** (ADR-435) delivers a signal sent to the runtime — `:term`,
`:int` (Ctrl-C) or `:hup` — to the calling process as the message
`[:signal sig]` instead of its default action (for all three, ending the
runtime). `(os/trap sig pid)` names another process; `(os/trap sig nil)`
gives the signal back what it did before it was trapped (the default, or the
`SIG_IGN` a `nohup` leaves). One subscriber per signal, and the call returns
the previous one. If the subscriber has exited when the signal arrives, the
signal does what it did before the trap, so a forgotten trap never leaves a
program unkillable.

**An application** (`std/proc/application.blsp`, ADR-436) is what a
long-running program starts, in what order, and how it stops:

```lisp
(defmodule shop (:use application))
(defapp store {:start (fn () (supervisor/start [...]))})
(defapp web {:start (fn () (supervisor/start [...])) :requires [store]})
```

`:start` takes no arguments and returns the application's root, usually a
supervisor's pid; `:stop` (optional) receives that root; `:requires` lists
applications that start first. When the manifest's `:main` names an
application rather than a function, `nest run` and a released binary
**run** it: start every requirement depth-first, each once, then the
application; trap SIGTERM and SIGINT; wait; and on the signal (or
`(application/shutdown)`) stop everything in reverse and exit 0. A pid root
with no `:stop` gets `gen/stop` — a supervisor answers by stopping its own
children — and is killed if it is still running after 5 s. If a root process
exits by itself, the rest stop, and `run` returns for a `:normal` exit and
raises with the reason otherwise. A start that raises stops what already
started, in reverse, and re-raises. `nest run --check-boot` starts the
application and stops it, running nothing else; the boot check `nest release`
runs on the binary it built only resolves it, since that runs on the build
machine, where a start would bind the application's ports.
`application/start`, `application/stop`, `application/run` and
`application/check` are the same steps by hand.

### Timers

`(timer/send-after ms pid msg)` delivers `msg` to `pid` after `ms` milliseconds;
`(timer/send-interval ms pid msg)`
delivers it every `ms` until cancelled. Both return a **timer handle** (the
timer's pid — a tiny green process parked on the scheduler's timer wheel, so a
pending timer costs no worker thread); `(timer/cancel h)` stops it, idempotently
(cancellation races an in-flight fire — a message already sent
stays sent). An interval timer monitors its target and exits when the target
dies, so a forgotten interval can't tick forever.

```clojure fragment
(def t (timer/send-interval 1000 (self) :heartbeat))
(receive (:heartbeat (redraw)))
(timer/cancel t)
```

### Blocking natives (`proc/offload`)

A long or blocking native — a git clone, a key derivation, big file IO — would
pin its scheduler worker for the whole call. `(proc/offload f & args)` runs it on
the **dirty-offload OS pool** instead (ADR-144): only the calling *process* waits (a selective receive under the
hood, so other mailbox messages stay queued), and errors rethrow at the call
site, catchable as usual.

```clojure fragment
(proc/offload file/slurp-bytes "big-archive.tar")     ; the worker keeps running others
(proc/offload %pbkdf2-sha256-bytes pw salt 600000 32)
```

Only long/blocking **data-in/data-out** natives are allowed (`%git-clone`,
`%git-resolve-ref`, `%git-list-tags`, `%untar-gz`, `%pbkdf2-sha256-bytes`, `%digest`, `%hmac`,
the compressors `%gzip`/`%gunzip`/`%zlib-compress`/`%zlib-uncompress`/`%deflate`/`%inflate`,
`file/slurp`, `file/slurp-bytes`, `file/spit`, `file/replace`, `file/spit-bytes`,
`file/spit-append`, `file/spit-bytes-append`, `%tls-self-signed`, and `%wasm-call`); anything heap-sharing or env-reading is refused with a
clear error. Args and the result are deep-copied across (like `send`), so
they must be sendable values. The package manager's clones already ride it.

### Per-process limits (`proc/flag`)

`(proc/flag flag [value])` reads or sets a runtime flag on the **current**
process and returns the previous — or, with no value, current — setting. The
first flag is **`:max-heap`**: a per-process heap limit in bytes.

```clojure
(proc/flag :max-heap 8000000)  ; cap this process at ~8 MB; returns previous
(proc/flag :max-heap)          ; read it
(proc/flag :max-heap nil)      ; clear it (also cancels a pending trip)
```

The limit is checked after each of the process's own GC collections against the
**live** (post-collection) footprint, so transient garbage never trips it. When
exceeded, the next safepoint raises a catchable error (`E0045`) **in that
process only** — uncaught, it kills just the offender, and every other process
(and the runtime) is untouched. That's the isolation the global
`BROOD_MEM_LIMIT` cap can't give: its hard tier aborts the whole OS process.

The global cap has two tiers. Crossing the **soft** limit (`BROOD_MEM_SOFT_LIMIT`,
default three quarters of the hard one) stops the process holding the most memory —
its heap plus its queued messages — with `E0043`, and names it on stderr; when that
is the process that noticed, the error is raised in it, catchably (ADR-399). A
**hard**-limit crossing — one allocation too large for the headroom — aborts the
OS process after one line naming the limit. With neither variable set, a process in
a memory-capped container (a cgroup `memory.max`) takes its hard limit from the cap,
at three quarters of it, so a runaway ends with `E0043` rather than an
out-of-memory kill that names nothing; elsewhere there is no cap unless you set one.
`BROOD_MEM_LIMIT=0` turns the limits off.

Policy stays in Brood: to spawn a capped worker, set the flag first thing in
the spawned fn —

```clojure
(spawn (fn () (proc/flag :max-heap 8000000) (work)))
```

**`:max-mailbox`** is the queue-length twin (ADR-308): a bound on this process's
mailbox, in messages.

```clojure
(proc/flag :max-mailbox 10000)  ; more than 10k queued messages is a fault here
(proc/flag :max-mailbox)        ; read it
(proc/flag :max-mailbox nil)    ; clear it (also cancels a pending trip)
```

Every sender checks the bound at enqueue, but **the sender is never blocked or
errored and no message is dropped** — `send` stays fire-and-forget, which is the
model's load-bearing rule (a blocking send re-introduces deadlock). A breach arms
a sticky flag that the *flooded process itself* raises as a catchable `E0046` at
its next safepoint or `receive` — the same one-process-only protocol as
`:max-heap`, with the same rescue: a handler can drain the backlog (`receive`
still works after the raise), clear or raise the bound, or let the supervisor
restart it. This is the guard against a slow receiver quietly eating the machine.
Backpressure proper stays a library concern: `gen/call` with a timeout is
already a bounded round-trip.

**`:responsive`** marks a process whose job is to keep answering — an editor's loop, a
REPL's reader (ADR-517). A native call that waits on the outside world (`os/cmd`, `os/pipeline`,
`os/run-process`, a clipboard read — ADR-536) is then reported to it or refused, instead of
silently freezing it:

```clojure
(proc/flag :responsive true)     ; report: after the wait, [:blocking-call {:native :detail :ms}]
(proc/flag :responsive :refuse)  ; refuse: a catchable E0048 before the wait — for a test
(proc/flag :responsive nil)      ; ordinary again; the read answers nil, :report or :refuse
```

The report arrives in the process's own mailbox behind whatever it already has, so a loop
handles it as one more event (bedit keeps it in its slow-turn log). The fix it points at is
always the same: move the call off the process, into a task, a port or a worker.

A file call (`file/slurp`, `file/spit`, `file/replace`, `file/ls`, `file/stat`,
`file/exists?` and the other `file/` natives) may wait too, on a network mount or a disk spun
down, but almost never does, so it is reported only when it held the process 20 ms or more,
and never refused (ADR-517's addendum): `[:blocking-call {:native "file/slurp" :detail
"/mnt/share/notes.txt" :ms 840}]`.

### Going idle (`proc/hibernate`)

`(proc/hibernate)` tells the runtime this process is about to wait a long time, so it
should give back everything it can: collect, shrink its heap, and drop its
inline caches and compiled-body cache. It returns the bytes released. It takes no
continuation: the process resumes from its own `receive`.

```clojure
;; a pooled connection that will sit idle between requests
(defn serve (conn)
  (proc/hibernate)                  ; ~40% smaller while parked
  (receive ([:request r] (do (handle conn r) (serve conn)))))
```

It is a **deliberate call, not a policy**, and the reason is measured: dropping
the caches automatically on every park costs message-heavy code 12–26%, because
a process that parks inside a loop needs the caches it just built. Idle
processes are the ones with something to give back, and only your code knows
which those are. So: use it in a process that will genuinely wait (a pooled
connection, an idle session actor, a supervisor between restarts); **don't** put
it in a request loop.

### Distributed nodes

Two runtimes (separate OS processes) can **connect over TCP and message each
other** — *the network is just a longer copy*. A **pid carries node identity**, so
the same value addresses a process whether it's local or on a peer; `send` routes
transparently.

```clojure fragment
;; node A: name the runtime, listen, expose a process by name
(node/start :a "127.0.0.1:9001" "a-shared-cookie-of-16+-bytes")
(proc/register :echo (self))

;; node B: connect, reach A's :echo by name, then talk to the pid it replies with
(node/start :b "127.0.0.1:9002" "a-shared-cookie-of-16+-bytes")
(node/connect "a@127.0.0.1:9001")
(send {:name :echo :node :a} [:hi (self)])
(def peer (receive ([:pong p] p)))   ; p is a remote pid
(send peer [:ping (self)])           ; addressed directly — location-transparent
```

| Form | Meaning |
|---|---|
| `(node/start name "host:port" cookie)` | Name this runtime and listen for peers. Returns the node name. |
| `(node/connect "name@host:port")` | Dial + authenticate a peer (shared cookie). Returns the peer's node name. |
| `(proc/register name pid)` | Bind a local name so peers can reach this process via `{:name name :node this-node}`. |
| `(node/name)` | This runtime's node name (`:nonode` until `node/start`). |
| `(node/list)` | A list of currently connected peer node names. |
| `(node/monitor name)` | Deliver `[:nodedown name]` when the link to `name` goes down (clean close or heartbeat timeout). |
| `(pid? x)` | True if `x` is a process id. |

The cookie is a shared secret of at least 16 bytes — anyone holding it has remote eval on the
node, so a shorter one is refused; `(node/cookie)` is a generated default (links are
encrypted — ADR-089). One
node per OS process. Remote `spawn`/code-shipping, distributed monitors/links,
heartbeat node-down detection, and mesh join have all shipped — full reference:
[distribution.md](distribution.md).

**Send semantics across a net-split:** `send` to a *disconnected* node silently
drops the message (the default). A process that must not lose messages opts
in with `(proc/flag :send-errors true)` — its sends then raise a catchable
`E0060` noconnection error, so it can queue and resend. Pair it with
**`reconnect`** (a `reconnect/…` reference auto-loads it): `(reconnect/watch spec)`
keeps the link alive with exponential-backoff reconnects, and
`(reconnect/subscribe spec)` delivers `[:nodedown name]` / `[:nodeup name]`
to your mailbox — resend the queue on `[:nodeup …]`.

## Builtins

> **Where these live:** only a small primitive kernel is implemented in Rust
> (the `%`-prefixed numeric ops, `cons`/`first`/`rest`, type predicates, I/O,
> `reflect/eval`/`reflect/load`, …). The functions below that aren't primitives — `+ - * / <
> = map reduce list …` — are defined *in Brood* in `std/prelude/*.blsp`,
> the same way you'd define your own. See spec.md §9 for the exact split. From a
> caller's point of view they're all just functions.

### Arithmetic
Bare in the prelude: `+`  `-`  `*`  `/`  `inc`  `dec`

In the `math` module: `math/mod`  `math/rem`  `math/quot`  `math/floor`  `math/min`  `math/max`  `math/zero?`  `math/nan?`  `math/infinite?`

> **The `math` namespace (ADR-227).** The derived math *library* lives in the `math`
> module: `sqrt`, `pow`, `ceil`, `round`, `round-to`, `clamp`, `abs`, `sum`, `product`,
> the sign/parity predicates (`positive?`, `negative?`, `even?`, `odd?`), and the
> constants `pi`/`e` — **and the non-operator arithmetic above**: `quot`/`mod`/`rem`,
> `floor`, `min`/`max` are derived too, so they moved here with the rest. Only the
> operators and `inc`/`dec` stay bare. Reach the library with `(:use math)`, which
> brings them back bare inside that module, or qualify (`math/sqrt`, `math/rem`).

- Integer-only arguments give an exact result: an integer, except that `/` gives a
  ratio when it does not divide evenly (`(/ 3 2)` → `3/2`, ADR-196). Any float
  argument makes the result a float.
- `(- x)` negates; `(/ x)` is the reciprocal.
- **Decimal arithmetic preserves scale** — `+`, `-` and `*` on exact operands
  (decimal/int/bignum) give the result the standard's *ideal exponent*: the finer
  of the two scales for `+`/`-`, the sum of them for `*`. So `1.50M * 2.25M` is
  `3.3750M`, not `3.375M`, and `(- 1M 0.0M)` is `1.0M`, not `1M` — significance
  survives the operation, which is the point of using a decimal for money. Only a
  zero *result* prints scale-less (`(- 1.50M 1.50M)` renders `0M`). A float operand
  anywhere makes the result an inexact float (contagion), and `/` is inexact by
  nature, so neither carries an ideal exponent. Pinned by the dectest conformance
  corpus (`tests/conformance_dectest_test.blsp`).
- Integer arithmetic never wraps: a result outside `i64` promotes to a bignum and demotes
  again when it fits.
- `math/rem` is the truncated remainder (sign of the dividend); `math/quot` is truncated
  integer division; `mod` is the euclidean remainder (always non-negative, in
  `[0, |b|)` — so `(math/mod 7 -3)` is `1`, not the floored `-2`).
- `math/floor`/`math/ceil`/`math/round` return an **int** (an int passes through unchanged);
  `math/round` rounds half away from zero. `math/round-to` keeps a fixed number of
  decimal *places* but stays a **number** (`(math/round-to 3.14159 2)` → `3.14`); for
  a fixed-width *string* like `"3.10"`, use `->fixed` (under Strings). `math/pow` requires an **integer exponent**
  (use `math/sqrt` for roots): an int base with a non-negative exponent stays an int
  (promoting to a bignum, like `*`); a negative exponent gives the reciprocal, which is
  **exact** for an exact base (`(math/pow 2 -1)` → `1/2`, and `(math/pow 2 -2000)` is exactly
  `1/2^2000` — no underflow) and a float for a float base. `math/sqrt` is always a
  **float**: it delegates to `%f64-sqrt` (`f64::sqrt`), so it is IEEE-correct
  including subnormals and ±0 — not the Newton's-method approximation it once was.
  All of these accept the whole numeric tower: an int, float, bignum, decimal or
  ratio argument coerces, and `math/floor` on a **ratio** is computed exactly rather
  than through f64 (so `(math/floor (/ a b))` is right past 2^53). A **bignum** past
  f64's range still roots correctly — `(math/sqrt (math/pow 10 400))` is `1e200`, not
  `inf` (the root is taken after an even power of two is factored out). `math/floor`,
  `math/ceil` and `math/round` of a finite float past 2^63 **promote to a bignum**, as `+`/`*`
  promote on overflow (ADR-393); only an infinity or NaN has no integer value and
  raises. `math/pow` is exponentiation by squaring (O(log exp) multiplications), and a
  result that would exceed 2^27 bits — `bit/shift-left`'s own cap — is refused as a
  catchable error rather than built: `(math/pow 2 100000000000)` used to run out of
  memory. The same ceiling bounds `(seq/repeat n x)` at 2^26 elements, the limit a
  range is realised under. `string/repeat` builds by doubling and refuses a result
  past a gibibyte.
- A source file may begin with a `#!` shebang line (skipped) or a byte-order mark
  (trivia); a control character inside a symbol is a parse error naming it. The
  shebang is a property of a whole source: `load`, `reflect/read-all` and
  `reflect/eval-string` skip it, `nest format` keeps it, and `reflect/read-string`
  of one form reads `#!` as the dispatch error it is. A line break is `\n`, `\r\n`
  or a lone `\r` — exactly what an editor counts, so a diagnostic's line is the
  editor's line; U+2028/U+2029 are ordinary whitespace, not line breaks. A
  `case`/`cond`/`match`/`receive` may have hundreds of arms: each lowers to one nested
  level per arm and the expansion ceiling is 1024 levels (ADR-395).
- `min`/`max` are variadic and require at least one argument. `even?`/`odd?`
  classify integers.
- Only `%add`/`%sub`/`%mul`/`%div`/`%lt`/`%eq`, `%rem`, and `math/floor` are Rust
  primitives; **everything in this section is Brood** on top of them
  (`std/prelude/core.blsp`) — including `+`, `<`, and `=` themselves.

### Bitwise
`bit/and`  `bit/or`  `bit/xor`  `bit/not`  `bit/shift-left`  `bit/shift-right`  `bit/count`  `bit/positions`

- Integer bit operations over an unbounded two's-complement representation, so a bignum
  works like an int. `bit/and`/`bit/or`/`bit/xor` are binary; `bit/not` is the unary
  complement (`(bit/not n)` = `(- (- n) 1)`).
- `bit/shift-left` never loses bits: a result past `i64` promotes to a bignum
  (`(bit/shift-left 1 64)` is `18446744073709551616`). `bit/shift-right` is an
  **arithmetic** (sign-preserving) shift, so `(bit/shift-right -1 200)` is `-1`. A
  negative shift amount, or one past 2^27, is a clean error, not a crash.
- `(bit/count n)` is the number of 1 bits (`(bit/count -1)` is `64`); `(bit/positions n)`
  is a vector of the set bit indices of a non-negative `n`, ascending (`(bit/positions 5)`
  is `[0 2]`).
- These are Rust primitives (they can't be bootstrapped from the numeric ops).

### Float bit patterns
`bit/float->`  `bit/->float`

- `(bit/float-> x)` is the IEEE 754 binary64 bit pattern of `x` as a non-negative
  integer — a bignum whenever the sign bit is set, since the pattern is a *u64*.
  `(bit/->float n)` is the inverse, for `n` in `[0, 2^64)`.
- This is **reinterpretation, not conversion**, and it is the only *exact* float
  comparison the language has. `=` on floats is value equality, which deliberately
  collapses `-0.0` and `0.0` and reports every NaN as equal to nothing:

  ```lisp
  (= -0.0 0.0)                                  ;=> true
  (= (bit/float-> -0.0) (bit/float-> 0.0))      ;=> false
  ```

- An `int` argument is taken as its float value, so `(bit/float-> 1)` and
  `(bit/float-> 1.0)` agree.
- Rust primitives: no bitcast or `frexp` exists to bootstrap them from. They are
  what the `parse-number-fxx` conformance corpus asserts against
  (`tests/conformance_parse_number_test.blsp`).

### Randomness
`rand/rng`  `rand/seed`  `rand/int`  `rand/float`  `rand/token`  `seq/shuffle`  `seq/sample`

- Brood has no global mutable state, so the PRNG is **pure and seedable**: every
  step takes a seed and returns `[value next-seed]`. Thread `next-seed` into the
  next call (carry it in your loop/process state like any other value). Seed a
  fresh stream from any integer — e.g. `(os/now)` — via `rand/seed`.
- `(rand/rng seed)` → `[value next-seed]` with `value` a non-negative 32-bit int;
  `(rand/int seed n)` → `[i next-seed]`, `i` in `[0, n)`; `(rand/float seed)` →
  `[f next-seed]`, `f` in `[0.0, 1.0)`; `(seq/shuffle seed coll)` →
  `[shuffled next-seed]`; `(seq/sample seed coll)` → `[item next-seed]`.
  `(rand/token n)` is the exception: `n` OS-random bytes as a hex string, no seed.
- The generator is Marsaglia xorshift32 — fast, fine for simulations, sampling,
  shuffling, jitter, and ids; **not** for cryptography. All of it is Brood over
  the bitwise primitives (`std/prelude/*.blsp`).

### Comparison & logic
`=`  ~~`not=`~~  `<`  `<=`  `>`  `>=`  `not`

- ~~`not=`~~ is **deprecated since 0.19.1 — write `(not (= a b))`.** It never bought a
  capability, only a spelling: its definition *is* `(not (= a b))`, and no part of the
  kernel, the compiler or the checker knows the name. It is also the slower of the two,
  structurally — `=`'s body is a direct primitive call, so the ADR-069 thin-wrapper elision
  collapses it, and `not`'s body is a leaf the inliner splices, while `not=`'s nested-call
  body is reachable by neither and stays a boxed call (measured 1.65x native, 1.38x on the
  VM, on a 5M-iteration loop). And its variadic reading misleads: `(not= 1 2 1)` is `true`
  — `=` chains pairwise and `not=` negates the whole chain, so the name reads "pairwise
  different" but means "not all equal". Every use is an *advisory* checker warning; the
  name still works and nothing is gated by it.
- `=` is structural and variadic (`(= 1 1 1)` → `true`). Numbers compare within
  their type (`(= 1 1.0)` is `false`); use `<`/`>` for cross-type numeric order.
  Integers compare exactly (no precision loss past 2^53), and floats compare by
  IEEE value — so `(= 0.0 -0.0)` is `true` and `(= nan nan)` is `false`.
- **`compare` is a total order over every value**, because `sort` needs one. It coerces
  across the numeric tower where `=` refuses to (`(compare 1 1.0)` is `0` while `(= 1 1.0)`
  is `false`), places NaN last, and orders collections by content: vectors and lists
  lexicographically, maps and sets by size and then by entries in key order. That last part
  is why `(sort maps)` sorts — until 2026-08-28 every map compared *equal* to every other,
  so the sort silently returned its input (ADR-285).
- A **record** is the exception, and deliberately so: it orders through its `compare-to`
  method or not at all — never by its underlying map layout. See
  [Multiple dispatch](#polymorphism-multiple-dispatch-defmulti) above.

### Lists & sequences
`cons`  `first`  `rest`  `second`  `third`  `last`
`list`  `vector`  `vec`  `conj`  `set/disj`  `into`  `seq`
`append`  `reverse`  `%reverse-onto`  `nth`  `count`  `empty?`
`range`  `take`  `drop`  `seq/split-at`  `seq/take-last`  `seq/drop-last`  `seq/take-while`  `seq/drop-while`
`includes?`  `any?`  `every?`  `seq/find`  `index-of`  `seq/zip`
`seq/partition`  `sort`  `seq/sort-by`  `seq/subvec`  `seq/reject`  `seq/remove-nth`  `seq/keep`
`seq/distinct`  `seq/flatten`  `seq/repeat`  `seq/repeatedly`  `seq/cycle`  `seq/prewalk`  `seq/postwalk`

- **`seq/cycle`** is bounded — `(seq/cycle [1 2] 5)` is `(1 2 1 2 1)` — because there is no
  lazy cons to make it infinite (ADR-111). **`seq/postwalk`/`seq/prewalk`** rebuild a nested
  value bottom-up / top-down through lists, vectors, sets and maps (a map's entries pass
  through `f` as `[k v]` vectors); `seq/prewalk` descends into what `f` produced, `seq/postwalk`
  does not (ADR-379).

> **The `seq` namespace (ADR-227; renamed from `enum` in ADR-234).** The higher-level,
> *derived* sequence helpers live in the `seq` module rather than the bare prelude: `dedupe`, `distinct-by`,
> `group-by`, `frequencies`, `chunk-by`, `chunk-every`, `interpose`, `interleave`,
> `scan`, `zip-with`, `reduce-while`, `min-by`, `max-by`, `enumerate`, `index-where`.
> The *core* protocol above (`map`/`reduce`/`fold`/`take`/`drop`/`sort`/…) stays bare;
> `take-while`, `partition`, `repeat`, `sort-by`, `mapv` and friends are `seq/` (ADR-506). **`filter` is not bare** — it is `seq/filter`, moved here in
> ADR-330 to sit beside its complement `seq/reject`; so are `distinct`, `zip` and `find`. Reach the helpers with `(:use seq)`
> in your module header, or call them qualified (`seq/group-by`). The descriptions
> below cover them all; bare spellings below need the import.

- `first`/`rest` of `nil` are `nil`. `nth` takes an optional default:
  `(nth coll i default)` — one value; a fourth argument is an arity error.
- **`seq/reduce-while`** is the early-terminating fold: `(seq/reduce-while coll init f)`
  where `(f acc x)` returns `[:cont acc']` to continue or `[:halt acc']` to stop,
  returning the last accumulator. `(seq/reduce-while (range 1000) 0 (fn (a x) (if (> a 100)
  [:halt a] [:cont (+ a x)])))` → `105`. An all-`:cont` reducer is just `fold`.
- **One sequence view, every collection.** `first`/`rest`/`last`/`count`/`empty?`/
  `map`/`seq/filter`/`fold`/`reduce`/`into`/`vec`/`seq` accept a list, vector, `bytes`,
  **set** (as its elements) or **map** (as its `[k v]` pairs) — so
  `(first {:a 1})` is `[:a 1]`, not an error, as it was before 2026-07-26.
  `(seq coll)` is the explicit coercion to that list view, and `(vec coll)` /
  `(into [] coll)` the vector form. `conj`/`into` insert at each kind's natural
  point and **preserve the kind** (vector→vector, set→set, map→map, list→list).
- `seq/enumerate` pairs each item with its index as `[i x]` vectors — the
  `map-indexed`/`keep-indexed` idiom is `(map (seq/enumerate xs) (fn ([i x]) …))`.
- `append` concatenates any number of sequences — lists *and* vectors, read as
  sequences — left to right, returning a **list**; wrap in `(into [] …)` for a
  vector. (The `concat` alias was removed — one spelling each.)
- `%reverse-onto` is `(append (reverse xs) ys)` in **one** pass instead of four, and
  is the spelling to reach for in a tail-recursive loop that has accumulated a
  reversed prefix and wants to splice it back in front of the remainder — the shape
  every non-tail-to-tail rewrite lands in. `ys` is shared, not copied, so only the
  prefix costs anything.
- `range`: `(range hi)` → `0..hi-1`; `(range lo hi)` → `lo..hi-1`;
  `(range lo hi step)` steps (ascending or descending); any other arity is an
  arity error. The result is a **lazy
  range** — an O(1) value that stands in for the list it denotes: it prints,
  compares (`=`), hashes, and `type-of`s exactly like that list, and
  `fold`/`reduce`/`math/sum`/`count` consume it in a counted loop with **zero
  allocation**; any other operation realises it to a real list on demand. An
  empty range is `nil`. `(range? x)` tests for the lazy handle (realised ranges
  are ordinary lists, so `range?` is false for them).
- `take`/`drop` clamp to the sequence length; `seq/take-last`/`seq/drop-last` take/drop
  from the end. `seq/take-while`/`seq/drop-while` split on the first element that fails
  the predicate. `seq/split-at` returns `[front back]` — the first `n` items and the
  rest — in a single pass (the fused `take`+`drop`).
- `any?`/`every?` return booleans (`every?` is vacuously true on the empty
  list); `seq/find` returns the first matching element, or `nil`.
- `index-of` returns the 0-based index of an element (by structural `=`), or -1;
  `seq/index-where` is its predicate counterpart — the index of the first item for
  which `(pred x)` holds, or -1.
- `seq/subvec` slices a vector, returning a **vector**: `(seq/subvec v start)` to the end
  or `(seq/subvec v start end)` for the half-open range `[start, end)` (the
  vector-preserving counterpart of `take`/`drop`, which return lists).
- `seq/reject` is the complement of `seq/filter` (ADR-330 — it was `seq/remove`, renamed
  because `remove` reads as element removal, which is what its neighbour `seq/remove-nth`
  actually does: that one drops the element at a given index, returning a vector for a
  vector and a list for a list). `seq/keep` maps a function and drops the `nil` results
  (map + filter fused) — note it collects `(f x)`, not the item, which is what makes it a
  different operation from `seq/filter` rather than a synonym. `seq/filterv`/`seq/rejectv`
  are the vector-returning pair.
- On a vector, `assoc`/`update`/`get` index by integer position — see
  [Maps](#maps) (`assoc`/`update`) and the index note there.
- `seq/distinct` removes duplicates, keeping the first occurrence (order-preserving);
  `seq/dedupe` collapses only *consecutive* runs of equal items.
- `seq/group-by` buckets items into a map from `(f x)` to the list of items that
  produced it. `seq/flatten` splices nested lists into one flat list (vectors/maps
  are leaves).
- `seq/interpose` inserts a separator between adjacent items; `seq/interleave` alternates
  two sequences, stopping at the shorter. `seq/zip` pairs two sequences into `[x y]`
  vectors, stopping at the shorter. `(seq/zip-with a b f)` combines two sequences element-wise via a
  binary function, the function last like every combinator. `seq/partition` chunks into `n`-sized groups, dropping a trailing partial
  chunk; `seq/chunk-every` keeps the remainder. `seq/chunk-by` partitions consecutive equal-key runs.
- `seq/scan` is a running fold — returns a list of all intermediate accumulator
  values starting with the initial value (like Haskell's `scanl`).
- `mapcat` maps a list-valued function and concatenates the results. `seq/min-by`/`seq/max-by`
  select the extremum of a collection by a key function. `(math/clamp x lo hi)` constrains a
  number to the closed range `[lo, hi]`.
- `seq/repeat` builds a list of `n` copies of a value; `seq/repeatedly` calls a
  zero-argument function `n` times and collects the results.
- `sort` orders ascending (or with a strict less-than predicate:
  `(sort xs >)`); `seq/sort-by` orders by a key function. Both are a **stable**
  merge sort. All of these are tail-recursive (stack-safe on long inputs).
- **Lazy, fusing pipelines.** `map`/`seq/filter`/`seq/keep`/`seq/reject` are **eager** — they
  return a concrete list and run their function immediately (so `(map xs f)` for
  side effects works). When you want a pipeline to **fuse** — fold/reduce in a
  single pass with no intermediate lists — use the lazy combinators `seq/lmap`,
  `seq/lfilter`, `seq/lkeep`, `seq/lreject`, threaded with `->`:
  `(-> (range n) (seq/lfilter math/odd?) (seq/lmap (fn (x) (* x x))) (reduce 0 +))`. Each returns a **lazy
  seq-view** — an O(1) value (like a [lazy range](#lists--sequences)) that stands
  in for the list it would produce. Chaining composes the stages onto one view,
  so the whole pipeline walks the source once, building nothing in between (≈3×
  faster than the eager form on large inputs). `(seqview? x)` tests for an
  unrealised view; consume one with `fold`/`reduce`/`math/sum`/`count`/`into`/`string/join`/
  `seq`/`first`, or realise it with `(seq v)` / `(into [] v)`. Two properties:
  like any lazy value a view defers its work (and any `throw` in its functions)
  until realised — **don't build a view for side effects**; use eager `map` or
  `doseq` for that — and a view is **heap-local**, so `send` refuses to ship one
  (realise it first) — it can't cross a process boundary or the network as a
  live view.

### Transducers
`seq/transduce`  `seq/xmap`  `seq/xfilter`  `seq/xreject`  `seq/xkeep`  `seq/xtake`  `seq/xtake-while`  `seq/reduced`  `seq/reduced?`  `seq/stage-slot`

The `l*` combinators above are the ergonomic front end; `seq/transduce` is the same
machinery with the stages exposed, for when the pipeline is **computed, reused, or
your own**:

```clojure
(seq/transduce (range 10) (comp (seq/xfilter math/odd?) (seq/xmap (fn (x) (* x x)))) 0 +)   ;=> 165   ; one pass
(seq/transduce (list 1 2 3) (seq/xmap inc) [] conj)                      ;=> [2 3 4]
```

A **transducer** is a function `(rf) -> rf'`, where a **reducing function** `rf` is
`(acc x) -> acc`; a stage wraps `rf` and may call it zero, one, or many times per
input. So a stage of your own is a plain `fn` — no protocol to implement:

```clojure
(defn xdrop-while (pred)
  (fn (rf) (fn (acc x) (if (pred x) acc (rf acc x)))))

(seq/transduce (range 6) (xdrop-while (fn (n) (< n 3))) [] conj)  ;=> [3 4 5]
```

There is one `comp`, and it composes right to left: `((comp f g) x)` is `(f (g x))`. Items
still flow through the stages **in the order they are written**, because a stage is applied
to a *reducing function*, not to an item: in `(comp (seq/xfilter p) (seq/xmap f))` the map
stage wraps `rf` first, the filter stage wraps that, and so the filter is the outermost
wrapper and sees each item first. `(comp (seq/xfilter p) (seq/xmap f))` filters, then maps
(ADR-507).

`(seq/transduce coll xform init rf)` is `(reduce coll init rf)` with the transducer
added: the collection first, the reducing function last.

**Early termination (ADR-376).** A stage that knows the rest of the input cannot matter
ends the run with `(seq/reduced acc)`: `seq/transduce` returns `acc` at once, and the source
is not walked further. `seq/xtake-while` is the built-in stopping stage:

```clojure
(seq/transduce (range 1000000) (seq/xtake-while (fn (n) (< n 3))) [] conj)   ;=> [0 1 2]   ; four stage calls
(seq/transduce (range 10) (fn (rf) (fn (acc x) (if (= x 4) (seq/reduced :early) (rf acc x)))) [] conj)
;=> :early
```

`seq/reduced` is a **non-local exit**, not a returned box: it throws a `%reduced` record
that `seq/transduce` catches and unwraps (`seq/reduced?` recognises the payload). That keeps
`fold`'s native loops free of a per-element test — a `try` costs ~100 ns per `seq/transduce`
call and the throw ~1 µs once, where a box test would cost every element of every fold —
and it means `seq/reduced` is a *transducer* protocol: outside a `seq/transduce` it is an
unhandled throw with a clear payload.

**Stateful stages (ADR-421).** A stage that counts, windows or remembers keeps its state in
the run's own table: each `seq/transduce` run creates one, binds it while the stack is built,
and drops it on every exit — the input ending, a `seq/reduced`, a throw. A stage takes a slot
with `(seq/stage-slot initial)` when it is applied to its reducing function and reads and
writes it with `table/get`/`table/put`; it never releases anything. `seq/xtake` is the
built-in one:

```clojure
(seq/transduce (range 1000000) (seq/xtake 3) [] conj)   ;=> [0 1 2]
(defn xdedupe ()                                           ; a stage of your own
  (fn (rf)
    (let ([run slot] (seq/stage-slot :none))
      (fn (acc x)
        (if (= x (table/get run slot)) acc (do (table/put run slot x) (rf acc x)))))))
```

There is no completion arity: Clojure's `(rf acc)` at the end would reach the caller's own
reducing function, where `(- acc)` negates. A stateful stage outside a `seq/transduce` raises.
The run's table costs ~0.45 µs per `seq/transduce` call.

### Maps
`hash-map`  `get`  `assoc`  `dissoc`  `contains?`  `keys`  `vals`
`merge`  `update`  `map/get-in`  `map/assoc-in`  `map/dissoc-in`  `map/update-in`  `map?`

The transformation helpers `reduce-kv`, `merge-with`, `update-vals`, `update-keys`,
`select-keys` and `zipmap` live in the **`map` module** (ADR-227) — `(:use map)` for bare
access, or call qualified (`map/update-vals`). The core protocol above stays bare.

See the [Maps](#maps) section above. `{ }` is the literal form; the rest are
immutable operations that return fresh maps. `count`/`empty?` work on maps too,
in **O(1)** — the CHAMP root node tracks its size (exposed by the `%map-count`
kernel primitive), so neither walks nor materialises the entries.

### Higher-order
`map`  `seq/filter`  `seq/reject`  `seq/mapv`  `seq/filterv`  `seq/rejectv`  `reduce`  `fold`  `apply`
`comp`  `partial`  `combinator/complement`  `combinator/constantly`  `identity`  `combinator/juxt`  `combinator/fnil`  `seq/pmap`

`combinator/juxt` applies several functions to the same arguments and returns the results as a
vector (`((combinator/juxt first count) [7 8 9])` is `[7 3]` — the two-field sort key); `combinator/fnil`
patches a `nil` first argument with a default (`(update m :n (combinator/fnil inc 0))`); `seq/pmap`
is `map` with `(f x)` for every item in its own process, results in order, a worker's
error re-raised in the caller (ADR-379).

There is no `memoize` function (ADR-522): a cache made by a call could be made per call, and
inside a loop each one was a table nothing dropped. Memoisation lives at a definition —
`(defmemo name capacity (params…) body…)` from the `memo` module defines a function that
remembers its last `capacity` calls, compared with `=`: bounded, and there is one per
definition by construction. Each process keeps its own store, holding the arguments
uncopied (ADR-670), so passing the very same value again is a hit by identity whatever its
size — a buffer, a 100,000-span vector — and a value only `=` to a remembered one still
hits after a walk. `memo/entries` reads the calling process's store.

```clojure
(map (list 1 2 3) inc)                      ;=> (2 3 4)
(seq/filter (list -1 2 -3 4) math/positive?) ;=> (2 4)
(seq/reject (list -1 2 -3 4) math/positive?) ;=> (-1 -3)
(seq/mapv (list 1 2 3) inc)                     ;=> [2 3 4]   ; vector result; `seq/mapv` beside `seq/filterv`
(seq/filterv (range 5) math/even?)          ;=> [0 2 4]   ; vector result
(reduce (list 1 2 3 4) 0 +)                 ;=> 10
(apply + (list 1 2 3))                      ;=> 6
```

`map`/`seq/filter`/`seq/reject` return lists; `seq/mapv`/`seq/filterv`/`seq/rejectv` are the
vector-returning variants for when the caller needs indexed access — the named form of
`(into [] (map …))`.

`reduce` takes `(reduce coll init f)` or `(reduce coll f)` (first item as the
seed, and raising on an empty collection, ADR-521); `fold` is the strict 3-argument form it wraps (`(fold coll init f)`) and is
what the prelude itself folds with. Both are public and both stay: `reduce` is the
surface you reach for, `fold` the strict-arity primitive it dispatches to (ADR-163).

The function combinators build the callbacks those ops take:

```clojure
(map (list 1 2 3) (partial + 10))                ;=> (11 12 13)  ; fix the leading args
(seq/filter (range 5) (combinator/complement math/odd?))    ;=> (0 2 4)     ; negate a predicate
(seq/reject (range 5) math/odd?)                 ;=> (0 2 4)     ; or just say reject
(map (list 1 2) (combinator/constantly :x))                 ;=> (:x :x)     ; ignore the argument
((comp inc (partial * 2)) 5)          ;=> 11          ; right-to-left composition
```

### Predicates
`nil?`  `pair?`  `list?`  `symbol?`  `keyword?`  `string?`  `number?`  `int?`
`float?`  `decimal?`  `bool?`  `fn?`  `vector?`  `map?`  `set?`  `ref?`  `range?`
`pid?`  `table?`  `bytes?`  `rope?`  `math/nan?`  `math/infinite?`

- `math/nan?` / `math/infinite?` classify the non-finite floats. They earn their place
  because `nan` and `inf` are *reader literals* (a bare `nan`/`inf`/`-inf` token is
  a float), so the language could produce them long before it could test for one:
  `=` reports NaN as equal to nothing, which is both the IEEE rule and the only
  way to detect it. `(math/nan? x)` says so by name.

- `(type-of x)` returns the runtime type tag as a keyword — `:int` `:float`
  `:decimal` `:string` `:symbol` `:keyword` `:bool` `:nil` `:pair` `:vector`
  `:map` `:set` `:record` `:failure` `:fn` `:macro` `:native` `:ref` `:pid` `:table`
  `:bytes` `:ratio` `:rope` `:socket` `:subprocess` — the spellings mirror the predicates above (the last
  two are opaque handles with no dedicated predicate). A lazy `range` reports
  `:pair`, since it stands for the list it produces. It's the reflective
  primitive that in-language type checks build on; the predicates are the
  common-case shortcuts.

### Strings

Every operation whose **subject is a string** lives in the `string` module
(ADR-230): call it qualified, or `(:use string)` to refer them bare.

`string/substring`  `string/char-at`  `string/->list`
`string/list->`  `string/->codepoints`  `string/codepoints->`
`string/upper`  `string/lower`  `string/capitalize`  `string/join`
`string/split`  `string/replace`  `string/trim`  `string/trim-left`  `string/trim-right`
`string/blank?`  `string/starts-with?`  `string/ends-with?`  `string/repeat`
`string/pad-left`  `string/pad-right`  `string/->graphemes`
`string/grapheme-count`  `string/grapheme-at`  `string/substring-graphemes`
`string/normalize`  `string/fill`  `string/fill-prefixes`  `string/fill-prefix`
`string/break-opportunities`

Bare in the prelude — these operate on *any* collection, so they are **not**
string-library ops: `str`  `pr-str`  `count`  `index-of`  `includes?`. A string's
length in chars is `(count s)`; there is no `string/length`.

Defined in the prelude but still qualified: `string/->number`  `string/->symbol`
`string/format`  `string/interp`  `string/display-width`. Number-to-text is `math/->fixed`.

- `str` concatenates the *display* form of its args; `pr-str` returns the
  *readable* form of one value.
- There is **no distinct character type** (deferred): a "character" is just a
  1-char string, so `(string/char-at s i)` and the elements of `(string/->list s)` are
  strings. All indices are **char-based**, matching `count` (so they are
  correct for multi-byte UTF-8, not byte offsets).
- `string/substring` and `string/char-at` are the char-indexed accessors, and `count` is
  the length they index against;
  `string/->list` / `string/list->` bridge to and from a list of chars.
- `string/->codepoints` gives the chars as a **vector of integer codepoints** in
  one O(n) native pass — the random-access form text parsers index with `nth`
  and compare as ints (`std/regex`/`std/json`/`std/encoding` all scan it);
  `string/codepoints->` is its inverse, and native too (`%codepoints->string`).
  It had no native form until 2026-08-26, so every one of those parsers rebuilt
  its result with `(apply str (map int->char cs))` — a one-character string and a
  closure call per code point, then an N-way concat. Building a string from code
  points is the normal way back, so reach for `string/codepoints->`.
- **A code point is not a character — a grapheme cluster is.**
  `string/->graphemes` gives the extended grapheme clusters (UAX #29) as a vector of
  strings: `"e"` + U+0301 is *two* code points but *one* cluster, and a flag emoji is
  four code points and one cluster. This is the unit to step a cursor by; stepping by
  code point splits a cluster and corrupts the text. `(apply str (string/->graphemes
  s))` is `s`. `string/display-width` counts terminal cells over the same clusters (a CJK
  char or emoji is 2, a combining mark 0), and `(string/width->index s cell)` maps a cell
  back to the character index of the cluster on it (`(count s)` past the end; a cell
  inside a wide glyph gives the glyph's start) — the pair an editor needs to place a caret
  and to map a click back to a character by the same measure. A **tab** is the one
  cluster whose width depends on where it is: it advances to the next tab stop (a
  multiple of 8 from the string's start), so `(string/display-width "a\tb")` is 9; both
  take an optional `start-col` (the column a mid-line chunk is laid out from, so its tabs
  land on the LINE's stops) and `tab-width`, and `(string/expand-tabs s [start-col
  tab-width])` is the string with each tab replaced by the spaces that reach its stop —
  what a view hands a frontend, since a raw tab in a render op has no column to measure
  from (ADR-342).
- **Cursor steps** (ADR-422): `(string/next-grapheme s i)` / `(string/prev-grapheme s i)` and,
  on a rope, `text/next-grapheme` / `text/prev-grapheme` (plus `text/forward-graphemes` /
  `text/backward-graphemes` for `n` steps) take and return *code-point* indices — the
  positions `string/substring` and the editor use — but always land on a grapheme-cluster
  boundary, so a cursor never stops inside `e` + a combining accent or an emoji sequence.
  The editor's `forward-char`/`backward-char`/`delete-char`/`delete-backward-char` and the
  line editor step by them. A rope step costs ~0.3 µs at any buffer size: the boundary is
  found over the rope's own chunks, with no segmentation built or cached.
- **The cluster-indexed accessors** are `string/grapheme-count`, `(string/grapheme-at s i
  [default])`, and `(string/substring-graphemes s start [end])` — the grapheme-indexed
  counterparts of `count`, `string/char-at`, and `string/substring`, which are all
  *code-point*-indexed. Reach for these whenever an index is a cursor position:
  `(string/substring s 1 2)` can slice a cluster in half (leaving a bare `e` and an orphan
  combining mark) where `(string/substring-graphemes s 1 2)` keeps it whole. Out-of-range
  reads yield the default and ranges clamp, exactly like `nth`/`take`. They exist so
  the correct spelling is also the fast one: `(nth (string/->graphemes s) i)` was the
  only way to read one cluster, and it materialised every cluster in the string on
  every keystroke (ADR-159).
- **Where a line may break** (UAX #14): `(string/break-opportunities s)` is the ascending
  character indices a line of `s` may end just before — after a space or a hyphen, between
  two CJK ideographs or kana (CJK has no spaces to break at), never before closing
  punctuation (`)` `」` `。` `，`) nor after opening punctuation (`(` `「`), after a URL's `/`
  but not before it. The break after a newline is mandatory and listed too; the end of the
  text is not. A word wrap fits what the width allows and ends the line at the last index
  that fits; a space-only wrap cuts CJK text anywhere and can strand a `」` at a line's
  start. The table and pair rules are `unicode-linebreak`'s, which cosmic-text already
  breaks lines with.
- **`=` is byte-structural, so text that reads identically can compare unequal**:
  `"é"` is U+00E9 *or* U+0065 U+0301. `(string/normalize s form)` normalises, with
  `form` one of `:nfc` `:nfd` `:nfkc` `:nfkd`. Canonical (`:nfc`/`:nfd`) preserves
  meaning; compatibility (`:nfkc`/`:nfkd`) also folds presentation — `"ﬁ"` → `"fi"`,
  `"²"` → `"2"` — which is what you want for search and identifier matching and not
  what you want for round-tripping text. Both are pinned by the UCD conformance
  corpora (`tests/conformance_ucd_test.blsp`).
- `string/upper` / `string/lower` case-fold (Unicode-aware: `(string/upper "ß")` → `"SS"`).
- `string/->number` is a **strict** parse — int if it is one, else float, else
  a **failure**; it rejects partial input (`(string/->number "3abc")` is a failure) and
  surrounding whitespace (`string/trim` first if needed). `str` is its inverse
  (the textual form of a number). An optional **radix** reads hex/octal/binary:
  `(string/->number "1F" 16)` → `31`. See [Text → number](#text--number) for the
  whole conversion story, including decimals.
- `index-of` returns the first char index of a substring or `-1`;
  `includes?` is the boolean form. `string/join` puts a separator between strings;
  `string/split` is its inverse (an empty separator splits into characters).
  `string/replace` swaps every occurrence of one substring for another.
- `string/trim` / `string/trim-left` / `string/trim-right` strip whitespace (both ends / left / right);
  `string/blank?` is true for an empty or all-whitespace string.
- `string/repeat` concatenates n copies; `string/pad-left` / `string/pad-right` justify a
  string into a fixed-width field with spaces (never truncating). `->fixed`
  renders a number with a fixed decimal count (`(math/->fixed 3.14159 2)` → `"3.14"`)
  — the float→text op `str`/`pr-str` can't do, since they print the shortest
  round-tripping form. Together they handle tabular/console output. `->fixed` is
  a Rust primitive (Rust's float formatter); the rest are Brood.
- `string/format` is a small `printf`-style wrapper: `(string/format "x = %d, y = %.2f" 42 3.14)`
  → `"x = 42, y = 3.14"`. Specifiers: `%s` (any, via `str`), `%d` (number),
  `%f` (float, 6 decimals), `%.Nf` (float, N decimals — uses `->fixed`), `%x`/`%X` (hex),
  `%%` (literal `%`). A width before any of them right-aligns in a field (`%8d`, `%10s`,
  `%8.2f`), `0` zero-fills (`%02x`), and `-` left-aligns (`%-10s`) — ADR-379:
  `(string/format "%-6s|%6.2f|%03d" "ab" 3.14159 7)` → `"ab    |  3.14|007"`.
- `string/interp` is **string interpolation** (a macro): `(string/interp "x = {x}, y = {(math/->fixed y 2)}")`
  splices each `{expr}` hole's value between the literal text, lowering to a plain
  `(str …)` — zero runtime cost, so it is just a terser `str`. `{{`/`}}` are literal
  braces; braces nest inside a hole (`(string/interp "m={ {:a 1} }")`). Prefer it over
  quote-chopped `str` wherever text interleaves values, including `error` messages:
  `(error (string/interp "index out of range: {i}"))`.
  An unknown specifier or a truncated one errors; a missing arg is `nil` (so `%s`
  renders `nil` and `%d` raises), extra args are ignored.

```clojure
(string/split "a,b,c" ",")      ;=> ("a" "b" "c")
(string/join (list "x" "y" "z") "-")   ;=> "x-y-z"
(string/replace "one fish two fish" "fish" "cat")  ;=> "one cat two cat"
(string/upper (string/trim "  hi  "))         ;=> "HI"
(string/->number "3.5")          ;=> 3.5
```

Only `string/upper`/`string/lower` (Unicode tables), `string/->number` (strict parse-or-failure),
`->fixed` (float formatting), and the O(n) char-access mechanisms
(`string/split`, `string/->codepoints`, `string/span`/`string/span-until`,
`%str-index-of` — one native pass each, where the pure-Brood equivalent is a
per-character loop) are Rust primitives; the rest of the library is Brood over
`string/substring`/`str` (`std/prelude/string.blsp`) — the "write the language in the
language" principle.

**Char indexing costs O(1), in both encoding regimes.** Strings are indexed by
Unicode scalar and stored as UTF-8, so every index has to be converted to a byte
offset. For pure-ASCII text the two numbers are equal. For multi-byte text the
conversion used to walk from the start of the string — which quietly made every
per-character or incremental-search loop O(n²) — and now goes through a sparse
char→byte index built on the string's first conversion (ADR-213), so it is a
lookup plus a bounded walk. `string/substring` is therefore O(result) rather than
O(index), and a `string/char-at` loop or an `index-of` scan with a rising `from` is
linear on any text.

### Text → number

Three functions, and which one you want depends on the *type you need*, not on the text:

```clojure
(string/->number "42")      ;=> 42      ; an int
(string/->number "3.14")    ;=> 3.14    ; a float
(decimal/of "1.50")         ;=> 1.50M   ; an exact decimal
```

**Text it cannot read yields a `failure`, not `nil`** (ADR-310) — a truthy value naming
the offending input, so it cannot vanish into a default unnoticed. `decimal/of` is the
exception and *raises*: a parse failing is data, a constructor failing is a bug.

```clojure
(error-message (string/->number "3abc"))   ;=> "string/->number: not a number: \"3abc\""   ; no partial parse
(failure? (string/->number "3abc"))      ;=> true
(def s "3abc")
(let (n (string/->number s))             ; defaulting, stated explicitly
  (if (failure? n) 0 n))                  ;=> 0
```

**`string/->number` decides int-vs-float from the digits**, so `"3"` gives you an `int`
even when you wanted a float. Force it with `->float`; go the other way with `math/floor`
or `math/round`, which both return an `int` — a bignum past 2^63 (there is no `trunc` —
`math/floor` of `-42.9` is `-43`):

```clojure
(->float (string/->number "3"))          ;=> 3.0
(math/round (string/->number "42.5"))    ;=> 43
```

**A radix reads hex, octal or binary from *data*.** In *source*, write the literal —
`0x1F`, `0b1010`, `0o17` (ADR-334). This is for text a program holds at runtime. With a
radix the parse is integer-only (in every base, 10 included), and it takes the digits
alone, no prefix:

```clojure
(string/->number "1F" 16)     ;=> 31
(string/->number "1010" 2)    ;=> 10
(string/->number "-ff" 16)    ;=> -255
(failure? (string/->number "0x1f" 16))   ;=> true   ; the prefix is reader syntax, not digits
```

A radix outside 2–36 **raises** — that is a bug in the caller, and a failure could not be
told apart from text that simply did not parse.

**`decimal/of` for money and anything that must not round.** `1.50M` is the literal form,
so reach for the function when the digits arrive as data:

```clojure
(+ (decimal/of "0.1") (decimal/of "0.2"))   ;=> 0.3M
(+ 0.1 0.2)                                 ;=> 0.30000000000000004
```

**The two fail differently, on purpose.** `string/->number` answers a failure for anything it
cannot parse and never throws — a parse failing is data, and testing it with `failure?` (as
above) is the idiom. `decimal/of` **throws** on malformed input, because a constructor handed
something that is not a number is a bug, not an outcome. Both are strict about the *whole*
string:

```clojure
(failure? (string/->number "  7 "))   ;=> true   ; trim first
(failure? (string/->number "1/2"))    ;=> true   ; a ratio literal is not a parse
(failure? (string/->number ""))       ;=> true
```

Its signature is `(string &optional int -> number | failure)`, so handing the result straight
to something that needs a number is what `nest check --strict` flags. The answer is the
`failure?` default above.

### I/O
`io/write`  `io/puts`  `io/inspect`  `with-out-str`  `with-err-str`

- `io/write` writes the display forms of its arguments to stdout (space-separated);
  `io/puts` adds a trailing newline and is the everyday one; `io/inspect` writes the
  **re-readable** form (strings quoted) plus a newline, for debugging. Each takes an
  optional trailing `:to <port>` — `(io/puts "boom" :to *err*)` is how stderr is
  written, so there is no separate `eprint`/`eprintln` family. All **flush stdout on
  every call**, so an animation frame paints immediately — there is no separate flush
  primitive (and none is needed).
- `(with-out-str body...)` evaluates `body` with stdout **captured** and returns
  everything it printed as a string (`""` if nothing), discarding `body`'s own
  value. Capture is process-scoped *and* inherited by any process `body` spawns,
  so a printer running in a child is captured too; and captures **nest** (the
  buffer is a stack), so a `with-out-str` inside another capture — e.g. a `nest
  mcp` tool handler, whose output is diverted off the JSON-RPC channel — drains
  only its own output. The buffer is released even if `body` throws (the error
  re-raises). Built on the `%capture-begin`/`%capture-take` kernel primitives.
- `(with-err-str body...)` is the stderr counterpart, and works differently
  because stderr does: `:to *err*` writes through the **`*err*` port**, so
  this rebinds that port to a collecting sink rather than using the kernel capture
  buffer. Two consequences follow — it captures only what goes through `*err*` (a
  diagnostic the *kernel* writes, e.g. `[reload] arity changed`, is not
  interceptable this way; that one has its own switch, `*reload-diagnostics*`), and
  it does not follow into spawned processes the way `with-out-str` does. Use it to
  **assert on a warning**, and to keep an expected warning out of a test run's
  output.
- For simple raw-terminal control, `(:use editor/ansi)` provides escape *strings*
  to `io/write`: `ansi-clear` (erase + home — the per-frame reset), `ansi-cursor`,
  `ansi-home`, `ansi-hide-cursor`/`ansi-show-cursor`. The ESC byte is the `\e`
  string escape. For a structured render-op frame buffer instead, use
  `editor/display` (`term/draw`/`term/emit`).

### Iolists (write-boundary trees)

The byte-producing write boundaries — `tcp/send`, `%proc-send`, `file/spit`,
`file/spit-append`, `file/spit-bytes`, `file/spit-bytes-append`, and the in-memory materialiser
`bytes/append` — accept any **iolist** (ADR-139): a
**string**, a **`bytes`** value, a **byte int 0–255**, or an arbitrarily nested
**list/vector** of iolists (`nil` is empty; an improper tail is a final leaf).
Describe the output as a tree and nothing is copied until the single flatten at
the write:

```lisp fragment
(tcp/send sock [status-line headers "\r\n\r\n" body])   ; no (str …) accumulation
(bytes/append ["ab" ["cd" nil "e"] (bytes 102) 103])    ; => #b"abcdefg"
```

String leaves are UTF-8 at text boundaries; a **binary**-mode socket/child
(`tcp/set-binary`) keeps its byte-string rule — each string leaf's codepoints
must be 0–255. Anything else as a leaf (a float, a keyword, an int > 255) is a
type error. This deletes the O(n²) `(str acc chunk)` accumulation class:
collect parts in a list and hand the tree to the boundary. `str`/`string/join` are
**not** iolist-aware — they render display forms (a list argument prints as a
list); use `bytes/append` (or `string/bytes->` of it) to materialise an
iolist in memory.

### Time & memory
`os/now`  `os/now-ns`  `dev/bench`  `dev/mem-bytes`  `dev/mem-peak`

- `(os/now)` returns wall-clock milliseconds since the Unix epoch as an integer.
  Subtract two readings to measure elapsed time — the test runner uses it to
  report how long a suite took. `(os/now-ns)` is the same in **nanoseconds**, for
  timing work too fast for millisecond resolution (i64 ns stays in range until
  2262).
- `(dev/bench "label" expr)` (a macro) evaluates `expr`, prints `label: N ms`, and
  returns `expr`'s value — drop it around any expression to time it in place.
- `(dev/mem-bytes)` returns the bytes currently allocated process-wide, and
  `(dev/mem-peak)` the high-water mark since the process started. They are fed by a
  byte-counting global allocator, so they cover *all* Rust allocations (the
  interpreter included), not just Brood values — which is what you want for
  "how much memory did this use." The test runner prints the peak alongside the
  time.
- `(dev/gc-stats)` returns a snapshot map of this process's garbage collection —
  `{:collections :copied :reclaimed :live :live-bytes :threshold
  :pause-total-us :pause-max-us :pause-last-us :debug-build :runtime-threshold
  :runtime-closures :runtime-aged :runtime-draining}` —
  for observing reclamation *and* pause behaviour: the `:pause-*` trio is
  cumulative wall time spent in this process's collections, the worst single
  pause, and the most recent one (µs). `:runtime-closures`/`:runtime-threshold` describe
  the shared code region instead, the same in every process. `:runtime-aged` counts the
  generations that region has aged and `:runtime-draining` says a drain of the aged one is
  still open (ADR-091, ADR-720): one that stays `true` names a process that will not let go.
  `:debug-build` is `true` when the binary carries debug assertions (i.e. *not* a performance
  build); `proc/info` carries the per-process `:collections` count too.
- `(dev/vm-stats)` returns the VM **work-attribution** counters — `:vm-apply`,
  `:tail-call`/`:self-tail`, `:tw-defer` (tree-walker fallbacks),
  `:call-ic-hit`/`:call-ic-miss`, `:global-ic-*`, `:prim1-*`/`:prim2-*`,
  `:env-get`/`:env-hops`, `:alloc`, `:jit-native`/`:jit-deopt` — answering *where the
  work goes*, i.e. whether the VM is dispatch-, env-, or alloc-bound. `:enabled` is
  `false` (and every other key absent) unless the binary was built with
  `--features perf-stats`, which `make perf-brood` does; the counters compile to
  nothing otherwise, so an ordinary build cannot attribute. `(dev/vm-stats-reset)` zeroes
  them: they are process-global and cumulative **from process start**, so a snapshot
  after a short program also counts the runtime's own boot — which is
  macro-expansion-heavy and defers to the tree-walker, enough to swing a defer rate
  from 0.8% to 84% purely on whether the boot cache was warm. A **counting** tool, not
  a timing one (the atomics perturb timing; read times from the counter-free benches).
  Policy lives in Brood: `(:use perf)` gives `(perf/report)`, `(perf/summary)` and
  `(perf/measure thunk)` — the last resets first, so it reports on that region rather
  than on the process. See `docs/benchmarking.md` and `docs/backend-seams.md` §5.
- `(dev/sched-stats)` returns the scheduler's cumulative counters —
  `{:spawned :exited :preempts :steals :migrations :workers :peak-threads}` —
  `:spawned − :exited` is the live-process figure, `:preempts` counts
  reduction-budget quantum exhaustions, `:steals`/`:migrations` count
  work-stealing activity.
- `(dev/profile-start [hz])` / `(dev/profile-stop)` — the **sampling CPU profiler**:
  arm at `hz` samples/sec (default 99), run the workload, and `dev/profile-stop`
  returns a histogram — a list of `{:stack (fn-names… innermost-first)
  :count n}` maps, most-sampled first. Sampling walks each process's reified
  call stack at its next VM frame boundary after every tick: no signals, and
  near-zero cost when off. (A JIT-resident loop is attributed when it yields
  at its reduction-budget preempt; the legacy tree-walker isn't sampled.)
- `(proc/system-monitor [pid opts])` — the **runtime event stream**: the kernel
  pushes selected runtime events to subscriber processes as ordinary
  `[:system kind subject-pid detail]` mailbox messages — `:gc` (a collection
  finished; detail `{:pause-us :collections :live}`, filtered by
  `:gc-min-pause-us`), `:spawn` (detail = parent pid), `:exit` (detail = the structured
  exit reason monitors see) or `:exit-abnormal` (only a reason other than
  `:normal`, filtered before any message is built), `:deopt` (detail = the
  JIT arm's fn name), and `:long-schedule` (a scheduling quantum held its worker
  at least `:long-schedule-us` µs — detail `{:us}`, ADR-404;
  selected by its threshold, and at 10 ms when every event is armed). **One subscription per subscriber pid** (ADR-305): no args
  reads the *caller's* subscription, `:all` lists every one, `nil` clears the
  caller's and `(proc/system-monitor nil pid)` clears `pid`'s;
  `(proc/system-monitor pid)` arms every event at `pid`,
  `(proc/system-monitor pid {:gc true :gc-min-pause-us 1000})` selects exactly
  the truthy keys. Events about a subscriber itself are never sent to it, and a
  subscriber's death drops its subscription. Off, the cost is one relaxed flag
  load per event site; a kind nobody selected costs the same. **Policy lives in
  Brood**: `(telemetry/watch-runtime [opts])` spawns a watcher that re-emits each
  kernel event as a `[:runtime kind]` telemetry event, so operators consume
  runtime and app events through one attach/handler seam, and the default crash
  reporter (`std/proc/crash-report.blsp`, below) is a second subscriber selecting
  `:exit-abnormal` — the two coexist, which the single-slot stream of before could
  not offer. A subscriber listening for abnormal exits also takes over the
  kernel's own `process N died: …` one-liner: while one is armed, the kernel
  prints nothing for an uncaught error (the `.brood_crash_dump` note is written
  regardless), so a crash is reported once, with its trace, by whoever asked.
- `(dev/gc-collect)` forces a collection now and returns the `dev/gc-stats` map
  (an observability/test aid, *not* a load-bearing trigger), and `(dev/gc-trace on?)`
  toggles per-collection stderr logging for the calling process (no arg = query;
  defaulted from `BROOD_GC_TRACE`). **Memory is reclaimed automatically:** the
  LOCAL heap is a **generational** copying collector (a nursery every `alloc`
  bumps into, plus a tenured old generation) that fires at the eval safepoint
  (ADR-055) whenever a process's live set crosses an adaptive threshold, or once it has made
  enough fresh vector, string and byte buffers (ADR-509: one object can own any number of
  bytes, so the object count alone is not enough). A minor collection copies the
  nursery's survivors and drops the rest, an occasional
  major compacts the old generation (ADR-072). So a long-running tail loop or
  `receive` server runs in bounded memory with nothing from the author — no
  manual GC call, no `while`, just recursion. (You never need to collect by hand;
  `(proc/hibernate)`, under [Going idle](#going-idle-prochibernate), is for a process
  about to sit idle, not for keeping memory bounded.) The three thresholds are tunable for a given workload via
  `BROOD_GC_FLOOR` / `BROOD_GC_TENURE` / `BROOD_GC_MAJOR` (object counts, `K`/`M`
  suffixes accepted).

### Metaprogramming / self-hosting
`reflect/eval`  `reflect/read-string`  `reflect/read-all`  `reflect/eval-string`  `reflect/load`  `reflect/undef`  `reflect/macroexpand`  `reflect/macroexpand-1`  `reflect/gensym`

There is **no user-facing `require` form** — you load an embedded standard-library
module by *referencing* it. A qualified reference `name/foo` auto-loads `name` on
demand (its other names stay qualified, `test/describe`); to refer them **bare**,
put a `(:use name)` clause in your `defmodule` header (see Namespaces), which loads
the module and refers its names. (The kernel's internal loader is `require-one`,
used by the `:use` machinery, the runtime bootstrap, and a few dynamic/cycle
cases — not something you call directly.) A module loads once per runtime: a process whose
reference finds it mid-load in another process waits for that load to finish, parked like
any `receive`, and never sees a half-loaded module.

```clojure
(reflect/eval (reflect/read-string "(+ 40 2)"))  ;=> 42
(reflect/read-all "(a) (b) (c)")         ;=> ((a) (b) (c))  ; every form, vs reflect/read-string's first
(reflect/eval-string "(def x 1) (+ x 1)") ;=> 2  ; read+eval all forms, last value wins
(reflect/load "some-file.blsp")          ;~> the file's last value, its definitions now in the global environment
```

`reflect/read-string` returns the *first* form in a string; `reflect/read-all` returns *all* of
them as a list (the read-half of `reflect/eval-string` without the eval) — the basis for
form-by-form tooling, e.g. an editor evaluating the last sexp before point. Both
raise on a malformed/incomplete form; `reflect/parse-source` is the lossless,
error-tolerant alternative (it yields a CST, used by the formatter).

These three are the seed of "edit the system while it runs": read code, evaluate
it into the live environment, replace definitions.

**Removing a definition: `reflect/undef`** (ADR-391). The inverse of `def` — for a tool
that rolls a reload back and must forget what the rejected version added:

```clojure
(reflect/eval-string "(defmodule shapes) (defn map (xs f) :mine)")
(reflect/undef 'shapes/map)   ;=> true   ; a binding went away
(bound? 'shapes/map)          ;=> false
(reflect/undef 'shapes/map)   ;=> false  ; nothing was bound
(reflect/undef 'map)          ;=> RAISED reflect/undef: `map` is a reserved name — it ships with Brood and cannot be removed
```

The symbol is the global's **full** name taken as data — `'shapes/map`, not a bare name
resolved against the current module. Everything recorded about the name goes too: its
privacy, `meta`, def site, declared `sig` (and a contracted function's module-boundary
alias), `defdyn` mark; its `%global-generation` moves. Afterwards a bare reference
*compiled from then on* resolves as though the name had never been defined — through the
module's `(:use …)` imports, then root — so removing a module's `map` lets its code reach
the prelude `map` again. Code **already compiled** while the name was bound (a VM arm, a
JIT'd arm, an arm in another process) never keeps the old value: removal invalidates every
cache of it exactly as a redefinition does, and the reference raises `unbound symbol: …
(removed by reflect/undef …)`; re-evaluating that code resolves its references afresh. So
a roll-back removes first and then reloads the good source, whose code then compiles
against the table as it now is. Macros already expanded are unaffected (expansion
happened at compile time). Inside a module load the removal is staged with the load's
definitions and published with them (ADR-344): the loader sees the name unbound at once,
everyone else when the load completes, and nobody if the load throws.

`reflect/load` does **not** remove the definitions a file no longer contains: which names
"belong" to a file is not something the runtime can know soundly (a name may be defined
from two files, from the REPL, or by a macro whose expansion changed), so a tool that
wants reload-with-forget diffs the old and new source's definitions and calls
`reflect/undef` itself, before it loads.

### Namespaces

A file opens a **namespace** with `(defmodule foo "optional doc")` as its first
form (`defmodule` *is* the namespace form; there is no separate `ns`). A file may
open **more than one** `(defmodule …)` — each opens a *region* running to the next
`defmodule` or EOF, and a bare reference qualifies against the module it is inside,
so co-located modules don't see each other's bare names (ADR-223); a single-module
file is the common case. Inside it, every `def`/`defn`/`defmacro` defines the **qualified** name
`foo/name`, and a bare reference resolves to `foo/name` when this namespace
defines it (including a *forward* reference to something defined later in the
file), otherwise it falls through to the **root** namespace — the prelude and any
non-namespaced globals. This keeps first-party and third-party code from
clobbering each other in the one shared global table (ADR-019/065), without a
separate namespace axis in the core: `foo/name` is just one interned symbol (`/`
is an ordinary symbol character), so the runtime, hot reload, and `send`/copy are
unchanged.

```clojure fragment
(defmodule text "buffer text ops")
(defn insert (buf i s) …)        ; defines text/insert
(defn append (buf s) (insert buf (len buf) s))   ; bare `insert` → text/insert
(map insert bufs)                ; `map` → root/prelude (not text/map)

;; from elsewhere — fully-qualified, and still openly redefinable:
(text/insert b 0 "x")
(def text/insert (fn …))         ; advice / hot reload works
```

**A module's own definition wins over a root name — function or macro — inside that
module** (ADR-423). `(defn when-let (x) …)` in a module makes every bare `(when-let …)` there
call it, not expand the prelude's `when-let` macro; `/when-let` still reaches the root one.
Because a root *macro* changing meaning is easy to miss, defining one's name prints a warning
at load — `warning: \`m/when-let\` shadows the prelude macro \`when-let\` …` — unless the header
declares the shadow: `(defmodule m (:shadow when-let))`. Shadowing a root *function* is silent:
modules owning `get`, `map` or `count` is the designed style (ADR-236).

The same rule cannot let a module capture a *macro's* expansion. When a macro is defined,
every call head in its quasiquote templates that names a root binding is written `/name`
— `when-let` expands to `(let (v x) (/when v …))` — so a module's own `when` or `first`
never captures what a prelude or library macro emits. Only call heads are escaped, and
never one spelled like a binder of the same template; an argument naming a root function
(`(map ~xs first)`) is written `/first` by hand.

**`/name` is the root binding under any local, too.** A macro places code the CALLER wrote
around code of its own — `match` binds your pattern variables around the walk that reads the
rest of the list — so a local the caller named `rest` used to capture the walk's `rest`
(`((x y & rest) op)` raised `cannot call non-function: nil`). The compile pass now renames any
local binder that would capture a `/name` in its scope (the renamed local still reads as
written in arglists and errors), so a pattern variable may be named `first`, `rest`, `get` or
anything else, and `(let (first 5) (/first xs))` calls the root `first`. Code that never passes
through the compile pass — a form a macro expanded lazily — reads `/name` from the global
table directly, which no local can shadow either. The prelude's code builders (the pattern
compiler, `for`, `receive`, `with`) write their root names `/name`, and
`tests/prelude_capture_test.blsp` holds every prelude function that builds code to it.

Import other namespaces' names with `(:use …)` clauses in the header. `(:use mod)`
refers all of `mod`'s public names bare; `(:use mod :only [a b])` refers just
those. A bare reference resolves **current namespace → imports → root**, so an
own-namespace definition shadows an import. `:use` auto-loads the module (it never
*fetches* a package — declared deps only). There is **no `require` form** — you load
a module by *referencing* it: a `(:use mod)` clause loads `mod` and refers its names
bare, and otherwise **a qualified reference `mod/name` auto-infers the load**
(ADR-227 follow-up) — naming where something comes from loads it on demand (its names
stay qualified, `mod/foo`), for *any* module. This holds for a qualified
macro head (loaded before it expands), a qualified value reference, and top-level
references in a header-less script or the REPL. **The load happens on first use, not when
the referencing file loads** (ADR-335): a qualified reference promises `mod/name` is bound
*when it is evaluated*, so a function reference loads its module at the first call that
reaches it (a macro head still loads before it expands, and so does a head into a module the
startup image does not describe, since only the image knows a name's kind without loading).
Consequences: `(bound? 'json/decode)` and `*features*` are false until `json` is first used;
a module's load-time effects — an `impl`, a `def-face`, a top-level side effect — happen at
first use; a broken or absent module errors at first use (the checker still reports both
statically). A file that needs a module loaded *before* its first call says so with
`(:use mod)` or `(:alias mod)`, which load at the referencing file's load as they always have.
There is **no bare-name magic**: a bare
`sqrt` with neither a `math/` prefix nor `(:use math)` stays unbound. The header understands exactly seven clauses — `(:use …)`,
`(:use-internals …)`, `(:alias …)`, `(:shadow …)`, `(:implements …)`, `(:check-allow …)` (see
[Type annotations](#type-annotations)) and `(:load a b …)`, which loads modules at this file's load
and refers nothing: the explicit eager request for code that must have a module BEFORE its first
call reaches it (ADR-335 — the test runner declares its whole dependency closure this way, since
a module first used inside an `:isolated` unit is rolled back with it); **anything else is a
hard error**. (It used
to be silently ignored, so a misspelled `(:use-internal m)` or a Clojure-style
`(:require m)` looked like it imported names or granted access and did nothing at
all — the worst failure mode for a header that governs imports *and* privacy. That
silence also hid four std modules whose `(:doc "…")` header dropped their module
docstring on the floor.)

```clojure fragment
(defmodule editor "the editor core"
  (:use editor/buffer)                 ; refer buffer's public names bare
  (:use text :only [insert]))  ; refer just text/insert as `insert`
(defn open (path) (insert (new-buffer) 0 (file/slurp path)))   ; insert → text/insert
```

**Ambient names are ambient by declaration, not by spelling** (ADR-151). A name
declared with **`defdyn`** is never namespaced: a `(def *load-path* …)` in any
module rebinds that one root binding, reachable bare everywhere (and so it must be
project-unique). **Every other name is namespaced, earmuffs included** — a plain
`(def *width* 10)` inside module `a` defines `a/*width*`, private to `a`'s
namespace like any other definition.

The earmuff spelling used to grant ambient status on its own, which made an
ordinary module-local constant silently global: module `a` and module `b` could
each write `(def *width* …)`, share one root binding, and the second load would
clobber the first with no diagnostic — `(a/a-width)` then returned *b*'s value.
Earmuffs remain the convention for a knob (and the checker still reads them as
one); they just no longer change scoping.

Two consequences worth knowing:

- To let other modules read or set your knob, declare it: `(defdyn *my-knob* v)`.
  A knob only its own module touches needs nothing.
- A **root** registry can only be rebound by root code, so the prelude exposes
  setters for the ones tooling extends — `(reflect/set-load-path dirs)` /
  `(reflect/add-load-path dir…)` for `*load-path*`, `(%record-module-doc! mod doc)` for
  `*module-docs*`. Writing `(def *load-path* …)` inside a module would define
  `mod/*load-path*` and the loader would never see it.

**Privacy is enforced** (ADR-146): **`defn-`** and **`def-`** define a
module-private name — identical to `defn`/`def` except the name is not exported.
The name itself is clean (`(defn- parse-line …)`, called `(parse-line …)`); privacy
is a property the def form declares, not a marker in the name. From inside *another*
module, a hand-written qualified reference to a private — plain or via an
`(:alias …)` — is a **compile error at load**, and `(:use mod :only […])` refuses to
import one. Three deliberate doors stay open:

- **`(:use-internals mod)`** in a module header is the explicit grant (the
  `@testable import` seam) — tests and tightly-coupled tooling declare their
  privileged access loudly; it also refers `mod`'s public names like `(:use)`.
  **`(:use-internals mod :only [a b])`** grants the same access but refers just
  the named symbols (which may be privates) instead of a refer-all. That is the
  **cycle-safe** form for two mutually dependent modules: a plain refer-all into a
  still-loading module mid-cycle is rejected by the loader, while a subset refer
  resolves lazily. The grant is emitted before the refer, so a private in the
  `:only` list passes the privacy check.
- **Top-level / REPL code** (no `defmodule`) is unrestricted — the
  live-hacking hatch: hot-reloading or advising a private from the REPL keeps
  working (`def` of a qualified private still rebinds it).
- **A module's own macros** may expand to its privates anywhere: enforcement
  reads the *hand-written* source, pre-expansion (macro templates live behind
  `quasiquote`, which the privacy walk skips) — the pattern the test
  framework's `describe`/`test` macros rely on.

Reflection (`reflect/eval`, `reflect/global-names`, `bound?`) still sees the flat table —
privacy governs what a module's source may reference, not what the live image
contains. `(reflect/private? 'mod/name)` reports whether a global is private.

Privacy is a **recorded per-module fact**: a `defn-`/`def-` records the qualified
name in a per-runtime private-set (via the `%mark-private` primitive it emits) that a
single kernel predicate (`Heap::is_private`, exposed as `reflect/private?`) consults. Because
a private name is spelled identically to a public one, **every** privacy check reads
the record — which means the defining module must be **loaded** to be judged. One
consequence: a qualified reference into a module that was *never loaded* can't be
judged private, so it degrades to a normal (call-time) **unbound reference** rather
than a load-time privacy error. That is deliberate — you can't assert privacy about
code the image has never seen. The prelude's own privates are seeded at build time
(the prelude is inserted, not re-evaluated).

The advisory checker additionally warns on private names that are
defined but never called within the file — see
[Advisory lints](#advisory-lints-non-type-warnings).
At the REPL the namespace tracks the last `defmodule`; `(reflect/current-ns)` reports it.

> Status: landed (ADR-065/066, 2026-05-30). `defmodule` is the single namespace
> form (`ns` removed); all of `std/` and every test file are namespaced; the
> checker, LSP, and `nest mcp` resolve names ns-aware. Macro templates
> **auto-qualify** their free references to the defining namespace (ADR-066 α), so
> a macro is robust across namespaces without hand-qualifying. Quoted symbols
> (`'foo`, message tags, map keys) are **never** qualified — they are data.
> A dependency's modules load under a `pkg/…` prefix, so package-level name collisions
> cannot happen (ADR-070).

### Introspection (editor tooling)
`reflect/doc`  `reflect/arglist`  `reflect/arglists`  `reflect/global-names`  `bound?`  `reflect/apropos`  `reflect/doc-search`

For self-description — the substrate an editor (and the language server,
`docs/lsp.md`) reads for hover, signature help, completion, and "is this name
known?". All derive from runtime state, so they stay correct as the program is
redefined.

```clojure
(defn add (a b & more) "Sum the arguments." (reduce more (+ a b) +))
(reflect/doc add)              ;=> "Sum the arguments."
(reflect/arglist add)          ;=> (a b & more)        ; mirrors the source surface
(reflect/arglists (fn ((a) a) ((a b) b)))   ;=> ((a) (a b))   ; every arm; reflect/arglist shows the last
(bound? 'add)          ;=> true   ; quote the name: bound? takes a symbol
(bound? 'no-such)      ;=> false
(includes? (reflect/global-names) 'map)  ;=> true      ; every global, for completion
```

**`bound?` is about names, not capabilities** — and for optional build features the
two disagree. A builtin from a feature this binary lacks is still bound and still
raises when called, so ask the build instead (ADR-197):

```clojure
(system/features)             ;~> [:gui :clipboard :jit :treesit :wasm :dev-tools]   ; this build's
(system/feature? :gui)        ;~> true    ; can this binary open a window at all?
(system/feature? :nonesuch)   ;=> false   ; unknown names are false, not an error
(bound? '%gui-open)    ;=> true    ; …even on a build with no gui backend
```

`system/features` reports what was **compiled in**, not what will work right now — a `gui`
build still fails on a headless box. It buys you the distinction between "this binary
lacks the feature" and "this feature failed", which is the one an app needs to decide
whether to degrade or to report.

For **discovery** — finding what exists rather than describing a name you
already know (the answer to "is there an RNG?" in one call):

```clojure
(reflect/global-names)           ;~> (… sorted list of every global …)
(reflect/apropos "rand")         ;~> (crypto/random-bytes … http/random-token)  ; loaded names containing "rand"
(reflect/apropos :shuffle)       ;~> (seq/shuffle)                      ; string/symbol/keyword pattern
(reflect/doc-search "random")    ;~> ([seq/sample "…"] [seq/shuffle "…"] …)    ; matches docstrings, not names
```

These three are Brood over `reflect/global-names`/`reflect/doc` (`std/prelude/tools.blsp`), and are
also exposed as `nest mcp` tools (`apropos`, `all-globals`, `doc-search`) so an
agent can explore the live image — see `docs/mcp.md`.

## Prelude

> **Reserved.** Every function and macro here is a *reserved name* — `(def map …)`
> is an error (ADR-166). Shadow one locally with `let`, or define your own inside a
> `(defmodule …)`; your own globals and your packages stay fully redefinable.

`std/prelude/*.blsp` — thirteen files concatenated in the order `boot.rs` lists —
is loaded at startup and is where most of the language
actually lives — the `defn` macro, the arithmetic operators, comparisons,
equality, the sequence library, and the `->` threading macro, all defined
in Brood on top of the Rust primitive kernel. It also adds `inc` `dec`
`identity` `second` `third` (`math/zero?`, `math/max`, `math/min`, the sign/parity
predicates, `math/abs`, `math/sum`, `math/product` and the rest of the math library are in the `math`
module — ADR-227). It's ordinary Brood — every function in it is defined with `defn`, exactly as you'd
define your own — but the *names* are reserved: it can be read, studied and copied,
not rebound (ADR-166).

## Standard library (opt-in modules)

These modules are baked into the binary but **not** loaded at startup — there is
**no `require` form**; reference a name qualified (`name/foo`, which auto-loads the
module) or add a `(:use name)` clause in a `defmodule` header to load one and refer
its names bare. Run `nest doc <module>` for the full API of any module.

| Module | name | What it provides |
|--------|---------------|-----------------|
| `std/file.blsp` | `'file` | Filesystem policy over the kernel's fs primitives: `read-lines`, `write-lines`, `spit-atomic`, `regular?`, `list-files`, `list-dirs`, `walk-files`, `copy-tree`, `move` (across filesystems too) and `trash` (the freedesktop.org trash, `~/.Trash` on macOS, the Windows recycle bin through `%file-recycle`). Pure path-string ops (`path/extension`, `path/stem`, …) live in `path` (ADR-234). Brood (ADR-006) but the recycle bin |
| `std/io.blsp` | `'io` | Output **ports** — the `Port` ability (`emit`), `stdout-port`, `stderr-port`, `process-port`, `file-port`, `port->fn`, and the `with-out`/`with-err` redirections — so output has a first-class destination instead of a fixed stdout. Also the writers themselves: `io/write`, `io/puts`, `io/inspect`, each taking an optional trailing `:to <port>` (see also `std/log.blsp`) |
| `std/text.blsp` | `'text` | The rope text surface over the `%rope-*` primitives: `string->`, `->string`, `insert`, `delete`, `slice`, `line`, `line-count`, `char->line`, `line->char`, the grapheme motions `next-grapheme`/`prev-grapheme`, `skip-forward`/`skip-backward`; a rope's length in characters is the bare `count`. (Greedy word-wrap is `string/fill`.) |
| `std/seq.blsp` | `'seq` | Derived **sequence helpers** (ADR-227) layered over the bare collection protocol: `dedupe`, `distinct-by`, `group-by`, `frequencies`, `chunk-by`, `chunk-every`, `interpose`, `interleave`, `scan`, `zip-with`, `reduce-while`, `min-by`, `max-by`, `enumerate`, `index-where`. Also `filter`/`reject`/`keep`/`filterv`/`rejectv`/`distinct`/`zip`/`find`/`flatten` and the transducer + lazy stages. The core ops (`map`/`reduce`/`fold`/`take`/`drop`/`sort`) stay bare in the prelude, and `take-while`/`partition`/`repeat`/`sort-by`/`mapv` are here (ADR-506); `(:use seq)` for bare access or call qualified |
| `std/map.blsp` | `'map` | Derived **map-transformation helpers** (ADR-227): `reduce-kv`, `merge-with`, `update-vals`, `update-keys`, `select-keys`, `zipmap`, and the path functions `get-in`/`assoc-in`/`update-in`/`dissoc-in` (ADR-506). The core map protocol (`assoc`/`dissoc`/`get`/`keys`/`vals`/`contains?`/`update`/`merge`) stays bare in the prelude; `(:use map)` for bare access or call qualified (the bare `map` *function* is unaffected) |
| `std/math.blsp` | `'math` | The derived **math library** (ADR-227): `sqrt`, `pow`, `ceil`, `round`, `round-to`, `clamp`, `abs`, `sum`, `product`, the sign/parity predicates (`positive?`/`negative?`/`even?`/`odd?`), and the constants `pi`/`e`. Only the **operators** (`+` `-` `*` `/` `<` `=` …) and `inc`/`dec` stay bare in the prelude — `quot`/`mod`/`rem`/`floor`/`min`/`max` are derived arithmetic and live here too, so they are `math/quot`, `math/min`, … unless `(:use math)` brings them back bare |
| `std/ansi.blsp` | `'ansi` | ANSI/VT100 escape sequences in program output: `strip` removes CSI colour/cursor sequences (reading a subprocess that emits colour), `decoder`/`feed` turn a stream into styled segments, split escapes held (ADR-567). For *emitting* escapes in a display frontend, see `std/editor/ansi.blsp` instead |
| `std/vt.blsp` | `'vt` | A **virtual terminal as pure data** (ADR-384): `(vt/feed vt chunk)` folds a program's output — cursor motion, erases, scroll regions, the alternate screen, SGR 16/256/true colour, the modes and queries an interactive program relies on — into a grid of cells; `(vt/screen vt palette)` reads it back as `{:text :spans :cursor …}` for a buffer to show, `(vt/line row palette)` a scrolled-off row with its colour, `(vt/key->bytes key vt)` an editor key — and `mouse->bytes` a click or wheel notch — as the bytes a program expects. The other direction from `std/term`: Brood *being* the terminal for a program under `os/spawn-pty` |
| `std/datetime.blsp` | `'datetime` | Gregorian calendar arithmetic over `datetime`/`date`/`time-of-day` records: `utc-now`, `today`, `add`, `diff`, `format`, `parse-iso8601`, `parse-date`, `parse-time`, `->epoch-ms`, `epoch-ms->`, the field readers |
| `std/tempo.blsp` | `'tempo` | **Time as an interval, not an instant** (adapted from Tempo, Apache-2.0 — see `docs/tempo.md`). One resolution-carrying value type whose span is half-open, so `2026-06` *is* `[2026-06-01, 2026-07-01)`: `new`/`parse`/`->iso` at any precision, `relation` (Allen's thirteen) with the predicates over it, an interval-set algebra (`union`, `intersection`, `difference`, `gaps`, `total-width`), unit-implied enumeration (`parts`) and `shift`/`next`/`prev`. The open `Spanning` ability admits your own types; `datetime/date` and `datetime/datetime` already impl it. Joins `datetime`'s `Temporal`, so one `->iso` covers all four temporal types. Layered on `datetime` for the calendar arithmetic |
| `std/encoding.blsp` | `'encoding` | Hex and Base64 encode/decode over strings (`hex-encode`, `hex-decode`, `base64-encode`, `base64-decode`) and byte vectors (`hex-encode-bytes`, `hex-decode-bytes`, `base64-encode-bytes`, `base64-decode-bytes`, plus URL-safe forms — byte-faithful, no UTF-8 round-trip) |
| `std/stats.blsp` | `'stats` | Descriptive statistics: `mean`, `median`, `variance`, `stddev`, `percentile`, `quantile`, `mode`, `min`, `max`, `range-of`, `sample-variance`, `sample-stddev` (`mode` uses `seq/frequencies` internally) |
| `std/stream.blsp` | `'stream` | Process-based pull streams (lazy, I/O-friendly): sources (`list->`, `range`, `socket->`, `fn->`, `singleton`), transformers (`map`, `filter`, `take`, `drop`, `chunk`, `lines`, `append`), terminals (`fold`, `each`, `->list`, `->vector`, `->socket`, `pipe`) |
| `std/url.blsp` | `'url` | URL encoding/parsing: `percent-encode`, `percent-decode`, `query-encode`, `query-decode`, `parse`, `build` |
| `std/csv.blsp` | `'csv` | CSV parse and emit: `decode`, `decode-maps`, `encode`, `encode-maps` |
| `std/uuid.blsp` | `'uuid` | UUID generation: `v4` (random), `v7` (time-ordered, RFC 9562), `v3`/`v5` (name-based), `zero`, `valid?` |
| `std/template.blsp` | `'template` | `{{var}}` string templating: `render`, `render-all` |
| `std/wasm.blsp` | `'wasm` | WASM component interop (ADR-071/145): `load`/`instantiate` a sandboxed component, `call` its exports (marshalled by WIT types, fuel-metered), `call-blocking` (the offload pool), `use-native` (bind every export as a Brood fn), `exports`, `close` |
| `std/queue.blsp` | `'queue` | Purely functional FIFO queue and min-priority queue |
| `std/multimap.blsp` | `'multimap` | Multi-valued map (one key → multiple values) |
| `std/sorted.blsp` | `'sorted` | Sorted map and sorted set, ordered by `compare` (ADR-430): `empty-map`, `empty-set`, `assoc`, `dissoc`, `get`, `contains?`, `min`, `max`, `floor`, `ceiling`, `range`. Collections (`seq`/`fold`/`into`/`conj`/`count`/bare `get` in key order), and two with the same contents are `=` |
| `std/hash.blsp` | `'hash` | `sha256`/`sha1`/`sha384`/`sha512`/`blake3`/`md5` (hex over strings or byte vectors), raw-byte digests (`sha256-raw` … → byte vectors, for chaining over bytes), `bytes->hex` (byte seq → lowercase hex), `hmac-sha256` (RFC 2104) and raw-byte `hmac-sha256-raw`/`hmac-sha1-raw`/`hmac-sha512-raw` (byte-vector key+msg → byte vector, for binary-protocol auth), `string` (djb2). All Brood over two Rust prims (`%digest`/`%hmac`). |
| `std/diff.blsp` | `'diff` | LCS-based sequence diff and unified diffs: `seq`, `lines`, `line-hunks`, `summary`, `patch`; `unified` prints the unified diff of two texts (`@@` hunks with context, `\ No newline` markers), `parse-unified` reads one, `apply-unified` applies one |
| `std/path.blsp` | `'path` | Path string manipulation: `join`, `split`, `basename`, `dirname`, `extension`, `stem`, `normalize`, `relative-to`, `absolute?`, `absolute`, `abbreviate` (`~` for `$HOME`, the inverse), `with-extension` |
| `std/sudo.blsp` | `'sudo` | A file read or written as another user through `sudo` (ADR-588): `read`, `write`, `password-required?`; the password validated once (`sudo -S -v`) and kept nowhere, the text on a `sudo -n` command's stdin; `*program*` names the sudo run |
| `std/system.blsp` | `'system` | The running program: `argv`, `script-args`, `halt`, `feature?`, `module-doc`. (OS interaction is `os/env`, `os/cmd`, `os/pipeline`, `os/run-process`; `file/cwd` is in `file`.) |
| `std/crypto.blsp` | `'crypto` | Cryptography: ChaCha20-Poly1305 AEAD (`encrypt`/`decrypt`/`encrypt-string`/`decrypt-string`), `password-hash`/`password-verify`, `pbkdf2` (accepts a string or byte-vector password/salt — a binary salt is used as raw bytes), `random-bytes`, `random-key`, `random-nonce`, `constant-time-equal?`, signatures and key exchange (`rsa-*`, `ecdsa-*`, `sign`/`verify`, `x25519-*`) |
| `std/proc/agent.blsp` | `'agent` | Process-backed state cell (an agent): `start`, `get`, `update`, `get-and-update`, `cast`, `stop` |
| `std/proc/names.blsp` | `'names` | A process registry (ADR-445): `start` (`:unique` or `:duplicate` keys), `register`, `unregister`, `lookup`, `whereis`, `count`; a `{:via registry :key key}` address works wherever `gen` takes a pid |
| `std/proc/groups.blsp` | `'groups` | Process groups across a cluster (ADR-446): `start` (one server per node), `join`, `leave`, `members`, `local-members` |
| `std/proc/application.blsp` | `'application` | Applications (ADR-436): `defapp`, `start` (requirements first), `stop` (reverse), `run` (until SIGTERM/Ctrl-C or `shutdown`), `check`, `app?`, `new` |
| `std/protocol.blsp` | `'protocol` | Behaviour contracts — the *module*-satisfies-a-contract seam: `defbehaviour` declares the ops a module must define (no value dispatch), claimed with `(:implements Name)` in a module header; `*protocols*` is the registry the checker and LSP read. Value dispatch is `ability` — `defprotocol`/`defimpl` were retired (ADR-168) |
| `std/telemetry.blsp` | `'telemetry` | Event instrumentation; handlers run in an isolated listener process: `start`, `stop`, `emit`, `attach`, `detach`, `detach-all`, `forward`, `handlers`, `sync`, the `span` macro, and the metrics and runtime watchers (`counter`, `summary`, `watch-runtime`, …) |

The following modules are also opt-in and live under `std/net/` and `std/tool/`.
Load each the same way — reference it qualified (`tcp/listen`, which
auto-loads) or `(:use name)` for bare names:

```clojure fragment
tcp      ; listen / connect / send / close … (thin wrapper over the net primitives)
http     ; fetch / post / request / serve / stream-response
sse      ; Server-Sent Events: connect (any method/headers/body, http or https) / frame / emit
test         ; describe / test / assert= / is — the test framework
format       ; printf-style string formatting
json         ; encode / decode
regex        ; match? / matches? / find / find-all / replace / split / tokens / paint,
             ;   rope-find / rope-find-backward / rope-count / rope-spans (a rope in place,
             ;   ADR-540), escape / incomplete? —
             ;   Brood's pattern dialect, translated for the native engine (ADR-389)
set          ; set-specific algebra: set / union / intersection / difference / subset?
             ;   (conj/disj/get/into/contains? on a set are prelude — no import needed)
fuzzy        ; fuzzy string matching
log          ; structured logging
task         ; promise-style async tasks over processes
eval-server  ; persistent image-isolated evaluator for a dedicated child
             ;   runtime: eval-server-run (stdin/stdout line loop) +
             ;   eval-capturing + the pure wire codec its clients share (ADR-198)
```

### Telemetry (`:use telemetry`)

An instrumentation seam (ADR-106), written in Brood. Code
**emits** a named event with a measurements map and a metadata map; **handlers**
attached to that event run on each emit — but in an **isolated listener process**, so
a handler can never affect the emitting process:

```clojure fragment
(defmodule my-app (:use telemetry) (:use log))

(telemetry/start)                                  ; spawn the listener once; supervise it

(attach :access-log [:http :request :stop]         ; id, event, handler
  (fn (event measurements metadata)
    (info (str (get metadata :method) " " (get metadata :path)
                   " → " (get metadata :status) " (" (get measurements :duration) "ms)"))))

;; Bracket work in a span: emits [:http :request :start] before, and either
;; [:http :request :stop] {:duration ms} on success or [:http :request :exception]
;; on a throw (then re-raises). Returns the body's value.
(span [:http :request] {:method "GET" :path "/"}
  (handle-request req))

;; Or emit a bare event yourself:
(emit [:cache :hit] {:count 1} {:key k})
```

The defining property — **telemetry can never crash the emitting process, only the
listener**:

- **Handlers run in a dedicated listener process.** `emit` is a fire-and-forget
  `send` to it, so a handler that throws, hangs, loops, or even hard-`exit`s can
  never crash or slow the process that emitted the event (e.g. a web-request
  process). The only casualty of a buggy handler is the listener — and a *throwing*
  handler doesn't even do that: the listener catches it and **detaches** it. An
  *uncatchable* fault (stack overflow, `(exit … :kill)`) kills only the listener;
  **supervise it** (it's an ordinary `:permanent` child) and it restarts with the
  handler table intact (the table is a separate global, ADR-013).
- **The trade-off.** Running handlers inline in the caller would be fast, but a bad
  handler would degrade the caller. Brood chooses total emitter isolation, at the
  cost of one listener as a serialization point. Keep handlers cheap, or use
  `(forward id event pid)` to ship events to a process you own and do the heavy work
  there.
- **Events are plain Brood values** compared by structural `=` — a keyword
  (`:request`) or a vector of keyword segments
  (`[:http :request :stop]`).
- **Zero-cost when off.** `emit` with no listener started is a cheap no-op.
- `telemetry/sync` flushes (a FIFO round-trip) — handy in tests and before shutdown.

`attach`/`detach` update a shared global, so call them at startup, not concurrently
from many processes (it is configuration-time). See `std/telemetry.blsp`.
