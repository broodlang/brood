# Brood language reference (v0.1)

This describes the language **as implemented today**. Anything not listed here
does not exist yet — see [ROADMAP.md](../ROADMAP.md) for what's coming (dynamic
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
| `(try … (catch Type e body))` | `catch` takes a **bare binding**: `(catch e body)`. There is no exception class. | A clean error naming the fix. (It used to bind the *class name* as the variable and evaluate `e` as a statement — and since the prelude defines `e`, `(catch Exception e (println "caught" e))` silently printed 2.718…, a wrong program with no diagnostic.) |
| Multi-arity `(fn ((x) …) ((x y) …))` | **Supported** (ADR-047) — dispatch by argument count, like Clojure. But param lists are **lists** `(x)`, not vectors `[x]`, and a clause head may *also* be a same-arity **pattern** (Erlang-style; see [Pattern matching](#pattern-matching)). | A **clean error** with a hint: vector-headed clauses `([x] …) ([x y] …)` are rejected. (They used to read as one 2-parameter pattern clause with an empty body — a completely different function, diagnosed only later as a misleading arity error at the call site.) |
| `{:a 1}` map literal | **Supported.** Immutable; `get`/`assoc`/`dissoc`/`keys`/`vals`/`contains?` (see [Maps](#maps)). Iteration order is **hash-derived, not insertion order** (unlike Clojure's array-map — see [Maps](#maps)). | Works as you'd expect, except don't rely on key order. |
| `{:keys [a b]}` / `:or` map destructuring | **Supported** — a map literal in pattern position binds each `:keys` symbol to the same-named keyword's value (nil if absent, or the `:or` default): `(let ({:keys [a b] :or {b 0}} m) …)`, works in `let`/`fn`/`match`. General `{:key subpattern}` nesting and `:as` are deferred (ADR-011). | Works as in Clojure for the `:keys`/`:or` subset. |
| `(defn f [x y] …)`, `(let [a 1 b 2] …)` | Param lists and `let` bindings are **lists** — `(x y)` / `(a 1 b 2)`. A vector *inside* one is still destructuring: `(let ([x y] p) …)`. | A clean error with a hint (ADR-149). The vector spelling was once accepted as an alias; that is what turned every Clojure binding shape into a silent misread — `(let [[a 1] [b 2]] …)` destructured `[a 1]` against `[b 2]` and reported `unbound symbol: b`. |
| `(let [[a 1] [b 2]] …)` / Scheme's `(let ((a 1)) …)` | Bindings are **flat**: `(let (a 1 b 2) …)`. | A clean error **with a hint** to flatten (was accepted-then-confusing). |
| `^:kw` / `^{:doc "…"}` metadata | Brood has **no metadata**. `^` is the pattern **pin**: `^expr` matches the current value of `expr` (ADR-150). | `^:kw` reads as a pin of the keyword — a pattern where you meant an annotation. Docstrings go in the body (see [Docstrings](#docstrings)). |
| `#{1 2 3}` set literal | A first-class set (`Value::Set`, ADR-060): `set?` true, prints `#{…}`, never `=` to a map. Evaluates its elements and dedups. | Reads as a kernel set; the `set` library (`(require 'set)`) adds `union`/`intersection`/… |
| `#(+ 1 %)` anonymous-fn reader macro | Write it out: `(fn (x) (+ 1 x))`. | A parse error **with a hint** naming `(fn …)` (was "unbound symbol: #"). |
| `#'foo` var-quote | Symbols are values — plain `'foo`. | A parse error **with a hint** naming `'foo`. |
| `#_form` discard reader macro | No form-level discard — wrap the form in `(comment …)`, whose body is read but never evaluated, or comment it out with `;`. | A parse error **with a hint** naming `(comment …)` and `;` (was a stray `#_` symbol). |
| `#"[0-9]+"` regex literal | Regexes are library values: `(require 'regex)` then `(regex/match? "pat" s)`. | A parse error **with a hint** naming `(require 'regex)` (was a stray `#`-symbol). |
| `\c` / `\newline` character literal | No character type — a character is a 1-char string `"c"` (or `(int->char 99)`). | A parse error **with a hint** naming the 1-char string (was `unbound symbol: \c`). |
| `#\|…\|#` block comment (Scheme/CL) | No block comments — comment each line with `;`, or wrap forms in `(comment …)` (read but never evaluated). | A parse error **with a hint** (ADR-169; used to read as a bar-quoted symbol). Any other `#…` is likewise reserved — `#` is a dispatch character, and `#{…}` / `#b"…"` are its only forms. |
| `1/2` ratio, `0x1F`/`0b1010` radix, `1_000` separators, `1N` bigint | None of these — a digit-led token must be a number Brood has. `(/ 1 2)` is a float, `0.5M` an exact decimal, `(string->number "1F" 16)` parses hex, `1000` needs no separator, plain `1` already widens to bignum. | A parse error **with a targeted hint** (ADR-169; these read as symbols before, surfacing as a far-away "unbound symbol"). Reserving the tokens keeps each future numeric syntax additive. |
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
| Integer | `0`, `42`, `-7` | 64-bit; arithmetic is overflow-checked. A result out of `i64` range promotes to an arbitrary-precision **bignum** rather than wrapping, and demotes back when it fits again — so the integer type is unbounded in practice. |
| Float | `3.14`, `-0.5`, `1e3`, `inf`, `nan` | 64-bit. **`inf`, `-inf` and `nan` are reader literals** — those three bare tokens are floats, not symbols, so they can't be used as names (the digit-required rule below has these three exceptions). Test them with `infinite?` / `nan?`; `=` reports NaN as equal to nothing, per IEEE. |
| Decimal | `1.50M`, `0M`, `-3.14M` | Exact arbitrary-precision base-10, for money and Postgres `numeric` — values a float can't hold (`(+ 0.1M 0.2M)` *is* `0.3M`). The literal is a trailing `M`; `(decimal x)` builds one from a string, int, bignum or float. Scale is significant in arithmetic (see [Arithmetic](#arithmetic)) but **not** in `=`, which compares values (`1.5M` = `1.50M`). |
| String | `"hello\n"` | Escapes: `\n \t \r \e \0 \\ \"` (`\e` is ESC, for ANSI terminal control), `\xHH` (two-hex-digit byte), `\u{H..H}` (1–6-hex-digit Unicode codepoint). A malformed `\x`/`\u{}` is a read error, and so is an unknown **alphabetic** escape (`\d`, `\w`, `\s`, …) — that's almost always a regex class written in a plain string, where dropping the backslash would silently break the pattern, so write `\\d`. A `\X` escape of punctuation or a digit (`\.`, `\/`, `\1`) is literal `X` (how you write a literal metacharacter in a regex string). Readable printing is the inverse: it re-escapes `\n \t \r \e \0 \\ \"` by name and any other control char as `\u{H..H}`, so a printed string always re-reads to the same value. |
| Symbol | `foo`, `+`, `my-fn`, `empty?`, `++`, `...` | Names; interned. **A token that leads with a digit — or a sign/dot immediately followed by a digit — must be a number** (ADR-169): if it isn't one Brood has (`1/2`, `0x1F`, `1_000`, `1N`, `1+`, `12-34`) it's a *reader error*, never a symbol, so those tokens stay reserved for future numeric syntax. A sign or dot with **no** digit behind it is not digit-led and stays a symbol — `+`, `-`, `...`, `.foo`, `foo.`, `--5`, `++`. A symbol whose name isn't a clean token — one built via `(symbol "a b")` with whitespace, delimiters, an empty name, or a spelling that would read as a number/keyword (including a reserved one, `(symbol "1+")`) — prints (readably) and reads back as a `\|…\|` **bar-quoted** symbol (`\|a b\|`, `\|1+\|`, `\|\|` for empty; `\|`/`\\` escape a literal bar/backslash), so every symbol round-trips through `pr-str`/`read`. |
| Keyword | `:ok`, `:else`, `:\|a b\|` | Self-evaluating named constants. Like symbols, a keyword whose name isn't a clean token (e.g. `(keyword "a b")`, `(keyword "")`) prints and reads as `:\|…\|`. |
| List | `(1 2 3)`, `()`, `(1 . 2)` | Cons cells; `()` is `nil`. Quote to keep as data: `'(1 2 3)`. A dotted tail `(a . b)` makes an improper list (round-trips with the printer). |
| Vector | `[1 2 3]` | A data type with O(1) indexing. Evaluates its elements. |
| Map | `{:a 1 :b 2}`, `{}` | Immutable key→value associations. Iteration order is hash-derived, **not** insertion order (see [Maps](#maps)). Evaluates its keys and values. Any value can be a key (compared structurally). |
| Function | `#<fn name>`, `#<native +>` | Closures and builtins. |
| Ref | `#<ref 0>` | A unique, opaque reference token from `(ref)` — no literal syntax; the only way to make one. Used to tag a request to its reply (see [Processes](#processes-concurrency)). |
| Pid | `#<pid a/7>` | A process id from `self`/`spawn`; carries node identity (`node/id`). No literal syntax. The location-transparent handle for `send` — local or across a node link (see [Distributed nodes](#distributed-nodes)). |

### Truthiness

Only `nil` and `false` are falsy. **Everything else is truthy**, including `0`,
`""`, and empty collections like `[]`, `{}`, and `#{}`.

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
> bare `(if x …)`. (`(if (seq x) …)` also works, since `seq` of an empty
> collection is `nil`.)

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
- **…but not of the language's own functions.** A name that shipped inside the
  `brood` binary — a prelude function or macro, a Rust builtin, or a function from an
  embedded std module — is **reserved**, and `(def get …)` is an error (ADR-166). The
  boundary is the binary: *if it came with Brood it is reserved; if you or a package
  author wrote it, it is yours*. Hot reload is untouched, because it was always about
  your code. This is the Erlang model — you cannot patch `Enum.map/2` on the BEAM
  either — and it is what lets the compiler bind those names early. Three things stay
  legal: a local `let` shadow (that binds a local, it isn't a redefinition), a
  module-scoped `(defn get …)` (which defines `your/mod/get`), and rebinding the
  prelude's *data* registries such as `*load-path*` (prelude functions do that
  themselves — the rule reserves shipped **functions**, not every shipped name). A
  **dynamic variable is never reserved**, whatever it holds: `defdyn` declares a name
  rebindable, so `(def *out* my-port)` — a permanent output redirect — still works
  alongside the scoped `binding` form.
- **No imperative loop.** There is no `while` (and nothing to make it progress
  without mutation). Iteration is **recursion** — proper tail calls give O(1)
  stack — or, for state that must evolve over time, a **process** (`spawn` /
  `receive`) that carries the state through its own loop.
- **Mutable state, when truly needed, is never a mutable `Value`.** It takes one
  of two shapes: a **process** holding evolving state in its receive-loop (the
  Erlang model), or a **Rust-backed opaque resource handle** exposed through
  primitives (the rope/buffer, or the [`table`](#in-memory-tables-broods-ets) store
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

## In-memory tables (Brood's ETS)

A `table` is shared, concurrently-mutable key→value state — Brood's answer to
Erlang's ETS (ADR-107). It's the escape hatch for the case immutable maps and
message-passing don't cover well: state that **many processes read and write
directly**, without routing every access through one owning process's mailbox. It is
a Rust-backed opaque handle (genuine mutable state, never a mutable `Value`):

```clojure
(def t (table))                  ; create — returns an opaque handle
(table-put t :hits 0)            ; store (overwrites); returns t
(table-get t :hits)              ; => 0   (a fresh copy)
(table-get t :missing :default)  ; => :default
(table-has? t :hits)             ; => true
(table-incr t :hits)             ; => 1   (atomic; default delta 1)
(table-incr t :hits 10)          ; => 11
(table-count t)                  ; => 1
(table-snapshot t)               ; => {:hits 11}  (an immutable map)
(table-delete t :hits)
(table-drop t)                   ; free it (else it lives till runtime exit)
```

How it behaves, and why it's safe:

- **Shared by identity, like a pid.** The handle can be `send`'d to (or captured by)
  other processes; every copy names the **same** store. `=` and `table?` compare by
  identity — two tables with equal contents are not `=`.
- **Copy-in / copy-out.** `table-put` stores a **deep clone** of the key and value;
  `table-get` returns a **fresh copy** into the caller. So no two processes ever alias
  a stored value (exactly ETS semantics), and the store is invisible to the garbage
  collector — it can't corrupt across a collection.
- **Keys are structural**, identical to map keys (`[1 2]`, `:k`, `"s"`, `42` all
  work).
- **`table-incr` is atomic** — a lock-held read-modify-write, so concurrent counters
  never lose an update. (There is deliberately no closure-based `update`: running
  arbitrary code under the lock can't be made safely atomic. For other atomic needs,
  serialize through a process.)
- **`table-snapshot`** is a consistent point-in-time map — unaffected by later
  mutation — and your enumeration primitive: use ordinary map ops (`keys`, `vals`,
  `reduce`) on it.
- **Local to the runtime.** A table can't cross a node link (send its
  `table-snapshot` — a plain map — instead). It lives until `table-drop` or runtime
  exit (no owner-death cleanup yet).

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
(`(sort (keys m))`) or compare via `frequencies`. Map equality (`=`) is itself
**order-independent**: `{:a 1 :b 2}` equals `{:b 2 :a 1}`.

Maps are immutable — every operation returns a **fresh** map:

| Form | Meaning |
|---|---|
| `(get m k)` / `(get m k default)` | the value at `k`; `nil` (or `default`) if absent. A **wrong-typed key** (a keyword into a vector/list/string) or a non-collection is a *type error*, not the default — `default` means "absent", never "malformed" |
| `(assoc m k1 v1 k2 v2 …)` | a new map with the pairs added/updated (also works on a **vector** with integer indices — replaces, never appends) |
| `(dissoc m k1 k2 …)` | a new map with those keys removed |
| `(contains? m k)` | whether `k` is present (distinguishes a stored `nil` from absence) |
| `(keys m)` / `(vals m)` | the keys / values, as a list, in the map's (hash-derived, unspecified) iteration order — sort if you need a defined order |
| `(reduce-kv f init m)` | fold over the entries: `(f acc k v)` left to right → the final acc |
| `(merge m1 m2 …)` | combine maps left to right; rightmost key wins (`nil` maps skipped) |
| `(merge-with f m1 m2 …)` | like `merge`, but a shared key's value is `(f old new)` |
| `(update m k f args…)` | a new map with `k`'s value replaced by `(f current args…)` (`current` is `nil` if absent; also works on a **vector** by integer index, which must be in range) |
| `(update-vals m f)` / `(update-keys m f)` | a new map with `f` applied to every value / key |
| `(select-keys m ks)` | a new map of just the entries whose key is in `ks` |
| `(zipmap ks vs)` | a map pairing `ks` with `vs` positionally (stops at the shorter) |
| `(get-in m path)` / `(get-in m path default)` | the value at a nested key `path`, or `default`/`nil` |
| `(assoc-in m path v)` | a nested copy with `v` stored at `path` (intermediate maps created) |
| `(dissoc-in m path)` | a nested copy with `path` removed (a missing path is a no-op; empty branches are left in place) |
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

### Keyword accessors

A **keyword is callable** — the one exception to "the head of a form is a function":

```clojure
(:name person)              ; ≡ (get person :name)
(:name person "unknown")    ; ≡ (get person :name "unknown")
(map :name people)          ; the reason for the exception
(sort-by :id procs)
(filter :cursor zones)
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
(:name #{:name})         ;=> :name        (membership, like get)
(:name nil)              ;=> nil
(:name deps)             ; type error: :name: expected a map, set or nil …
```

The advisory checker knows the form (ADR-167): it flags a receiver that provably
can't be keyed, a wrong arity, and flows a **typed record field** through the result —
so `(string-length (:x p))` is caught for a `(defrecord pt ((x int) …))` exactly as
`(string-length (get p :x))` is.

**Only keywords.** A map, vector or set in head position is still an error with a
hint — `({:a 1} :a)` would be a second spelling of `get`, and a callable vector or set
answers by index-or-membership, the ambiguity `contains?` deliberately refuses
(ADR-156/165). Use `(get m k)` when the key is *computed*; `(:k m)` can only ever
mean the literal keyword `:k`.

A map is **seqable as its `[k v]` pairs**: `seq`, `first`, `rest`, `last`, `map`,
`filter`, `fold`/`reduce`, `into`, and `vec` all read it that way, so
`(map first m)` is its keys and `(first m)` is a `[k v]` vector (`nil` for an empty
map). Use `reduce-kv` when you want the key and value as separate arguments.

These are thin Brood wrappers (`std/prelude.blsp`) over a small kernel of `map-*`
primitives; the representation is a **CHAMP hash-array-mapped trie** (ADR-040),
which is why `count` is O(1) and structural key equality is O(log n).

### Records

`defrecord` names a map shape and gives it a **nominal identity**. There is no new
value kind — a record *is* a map underneath, so every map operation above still applies
— but the constructor bakes in a reserved `:__id__` field holding a `:module/name`
keyword, so `ability` dispatch tells a record apart from a bare map and from other
records. `(defrecord point (x y))` defines a positional constructor and one accessor per
field:

```lisp
(defrecord point (x y))
(def p (point 3 4))      ; => {:__id__ :<ns>/point, :x 3, :y 4}
(point-x p)              ; => 3             — accessor, one per field
(assoc p :x 9)           ; => a fresh record {…:x 9 :y 4}, id and all
(fields p)               ; => {:x 3 :y 4}   — the clean, id-free view
(record? p) (record-id p); => true, :<ns>/point
(= (point 1 2) {:x 1 :y 2})   ; => false    — a record is NOT = a bare map (nominal)
(= (point 1 2) (point 1 2))   ; => true     — same shape + same id
```

The accessors are the reason to reach for it: `(point-x p)` names the field, and a
typo `(point-witdh p)` is a call to an **undefined function** — caught by `nest check`
and at runtime — whereas `(get p :witdh)` silently returns `nil`. Records are **nominal,
not structural** (Elixir-struct semantics): a record is never `=` to a bare map with the
same fields, and `record?`/`record-id`/`fields` are the identity API. Functional update
is plain `assoc`/`merge` (the id rides along).

A field may carry a type — `(defrecord point ((x int) (y int)))` — and when every
field is typed, `(sig …)` declarations are emitted for the constructor and
accessors, so the advisory checker (and `BROOD_CONTRACTS=1` runtime contracts) see
the field types. See [ADR-130](decisions.md) and `docs/types.md` for the record
type `(record :k T …)` this lowers to.

### Polymorphism: abilities

`defrecord` names a map's *shape*; an **ability** names an operation that different
types implement differently. Abilities are **core** (in the prelude, ADR-168/172):
open generic functions where each op dispatches on the **identity of its first
argument**, and an implementation can be added for any identity — including a built-in
kind — from any module, at any time, without editing the dispatcher.

`defability`/`impl`/`defrecord` are built in — always available, no import, no
`(:use ability)`.

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
  (area [self] :-> float))

(impl Shape geometry/circle (area [c] (* 3.0 (get c :r) (get c :r))))
(impl Shape geometry/rect   (area [r] (* (get r :w) (get r :h))))

(area (circle 2))         ;=> 12.0
(area (rect 3 4))         ;=> 12
```

Built-in kinds work the same way, with **`:default`** as the fallback:

```clojure
(defability Size (size [self] :-> int))
(impl Size :int      (size [n] n))
(impl Size :string   (size [s] (count s)))
(impl Size :default  (size [_] -1))

(size 7) (size "hello") (size 1.5)     ;=> 7, 5, -1
```

**Op arity.** An op dispatches on its **first** argument, so it must take at least
one — a zero-arg op `(op [])` is a clean expansion-time error. Beyond the first, an
op may take more fixed arguments (`(fetch [self k])`) or a `&`-rest
(`(cat [self & rest])`, dispatched on `self`, the rest passed through). An `impl`
must match the declared op's arity: a fixed impl exactly, a variadic impl as long
as it accepts that arity — `nest check` and load-time registration both flag a
mismatch.

**Single dispatch — and its multiple-dispatch sibling.** An *ability* op dispatches on the
**first argument only** (Rust-trait / Clojure-protocol style). For a unary op (`to-str`,
`to-seq`) that is all there is to it. An op that *combines two values* — arithmetic and
ordering — needs to see **both** operand types, which single dispatch cannot express
symmetrically (`(+ money 5)` could dispatch on `money`, but `(+ 5 money)` cannot). So those
live in the **`defmulti` multiple-dispatch seam**, not in an ability: `Num`
(`num-add`/`num-sub`/`num-mul`/`num-div`) and `Ord` (`compare-to`) are **multimethods**
dispatching on the operand *pair* (see [Multiple dispatch](#polymorphism-multiple-dispatch-defmulti)
below, [ADR-179](decisions.md)). Author `(defmethod num-add [money money] …)` and
`(defmethod num-mul [money :int] …)`; `+`/`*` are commutative, so the mirror is derived and
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

> **The `impl` dispatch id resolves to the way `identity-of` produces it.** `impl`
> and `:sealed` share one helper (`ability--id-kw`): a **bare** record symbol is
> qualified against the current namespace (`circle` → `:<ns>/circle`), an
> already-`/`-qualified symbol is used as written (`geometry/circle` — the form to
> use for a record from *another* module), and a keyword id (`:int`, `:default`) is
> left untouched. So a same-module `(impl Shape circle …)` and `(impl Shape
> geometry/circle …)` register under the same id a value presents; the earlier
> bare/qualified asymmetry is fixed ([KI-15](known-issues.md)).

A missing implementation is a **loud, named error** — `ability Shape/area: no impl
for :geometry/circle — have (…)`, listing the ids that *are* implemented — never a
silent `nil`. The thrown value is a **structured map**, `{:kind :no-impl :message …
:ability :op :id :have}`, so a handler can branch on the parts —
`(catch e (get e :id))` — rather than parse the string; `error-message` still
returns that same human text.

**Records dispatch as themselves; plain maps do not.** `(type-of r)` is still `:map`
for a `defrecord` value and `get`/`assoc` still treat it as a map — the nominal id is
carried in a reserved `:__id__` field (a record is *not* `=` to a bare map, though). A
`defrecord` value dispatches on its `:module/name` id; a plain map, **even one carrying
a `:type` field**, stays `:map` and lands on the `:map` impl. That is the ADR-011 line —
the identity is explicit and construction-time, never inferred from a field.

> **On-ramp: reach for `defrecord` *before* you want polymorphism.** Because dispatch is
> nominal, there is no structural path — you cannot `impl` an ability for "a map whose
> `:kind` is `:circle`". If you are building a value on plain maps told apart by a tag
> field and later want to dispatch on the kind, the move is to make it a `defrecord` first
> (its predicate `circle?` and its `:__id__` then carry the identity), then `impl` against
> that. This is a deliberate constraint, not a gap — it keeps identity construction-time
> and explicit — but it lands as a small refactor if you defer it, so prefer records for
> any value you expect to dispatch on.

**A driver is just a value.** Because dispatch is on the first argument, "swap the
backend" needs no config indirection and no module-atom dispatch — you pass a
different value:

```clojure
(defmodule store)

(defability Store (fetch [self k]))
(defrecord pg  (pool))
(defrecord mem (data))
(impl Store store/pg  (fetch [db k] (str "pg:" (get db :pool) "/" k)))
(impl Store store/mem (fetch [db k] (get (get db :data) k)))

(fetch (mem {"a" 1}) "a")          ;=> 1
(fetch (pg "main") "a")            ;=> "pg:main/a"
;; swapping the driver value swaps the impl — no config, no module atoms
```

**Sealed abilities.** `:sealed [id …]` names the **closed** set of ids the ability
is meant to cover, and `nest check` then flags any member missing a direct impl of
any declared op (a `:default` does not count). Runtime dispatch is unchanged —
sealing is a contract, not a restriction:

```clojure
(defability Shape :sealed [circle rect] (area [self] :-> float))
```

**Introspection and helpers.** `(satisfies? 'Shape x)` is true when every declared
op resolves for `x` (directly or via `:default`), so a caller can branch instead of
letting the op raise. `(record? x)` / `(record-id x)` / `(fields x)` are the clean
view of a record — `fields` returns the field map with the internal id omitted.
`(ability-ops 'Shape)` exposes the declared op specs as data, the same hook the
checker and LSP read.

The id is held in a reserved `:__id__` field, reachable by direct `(get r :__id__)`,
but the record's **collection view is the fields, id-free**: `seq`/`count`/`keys`/`vals`
— and `map`/`filter`/`fold`/`for`/`into`, which coerce through `seq` — see only the
fields, so `(count (circle 2))` is `1`. This is the `Seqable` ability (op `to-seq`,
default = the fields); a custom-collection record overrides it to define its own
iteration. `(fields r)` gives the id-free map explicitly; nothing else should read
`:__id__` directly (the stable seam a future hidden slot would swap behind).
A record being `≠` a bare map with the same fields, and printing with its id, are
*intended* (Elixir-struct semantics), not leaks; `json-encode` omits the id, so it
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
  [self] :-> float)` types as `float`, so `(+ (area s) 1.0)` checks and `(string-length
  (area s))` is flagged. This is the single most useful place to recover type
  information, because it's the polymorphic boundary where it would otherwise be lost.
- **Each `impl` body is checked against it.** An impl of `(size [self] :-> int)` whose
  body provably yields a non-`int` (a string literal, a `bool`) warns. The check is
  **gradual and false-positive-clean** — exactly like a `sig` return: an
  over-approximated body (a call result, an unknown param) is `dynamic` and defers; only
  a body *provably disjoint* from the declared return warns. A `:-> any` op imposes no
  constraint (`any` is the gradual unknown, not a top type). All advisory — the return
  type never gates the live image, and dispatch is unchanged.

**A sealed ability *is a type* (ADR-181).** A `:sealed` ability names a closed member set,
so its name is usable directly in the type grammar — it denotes the **union of its members'
record shapes**. `Shape :sealed [circle rect]` means `(or circle rect)` as a type, so you can
write `(sig total (Shape -> float))` or return one from an op (`(scaled [self] :-> Shape)`).
A member record satisfies it (records are open, so extra fields are fine); a non-record is a
provable mismatch; anything whose type isn't pinned down defers — sound, no false positives.
Only *sealed* abilities resolve this way (an open ability has no finite member set to
enumerate); an open ability's name in type position stays unknown and the annotation is
dropped rather than guessed.

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

Prefer `match`/`cond` when the set of cases is closed and local; reach for an
ability when third-party or later code must be able to add a case.

#### The display protocol: customizing how a record prints

The `Display` ability — Elixir's `String.Chars` for Brood (ADR-171/172) — is **core
and always on**. Its op **`(to-str x)`** turns a value into its display string; the
`:default` impl is the native `str`. The **screen printers** (`print` / `println` /
`eprint` / `eprintln`) route a *record* through its `Display` impl out of the box — no
`(require 'show)`, no activation step. Built-ins are unchanged and pay no dispatch cost.

```clojure
(defmodule money)             ; no (:use ability), no (:use show) — both are core
(defrecord usd (cents))
(impl Display usd
  (to-str [m] (str "$" (to-fixed (/ (get m :cents) 100.0) 2))))

(println (usd 1050))          ; => $10.50   (not {:__id__ :money/usd, :cents 1050})
(to-str (usd 1050))           ; => "$10.50" — the explicit call, for use inside str/fmt
```

Scope is the screen printers; `str` / `pr-str` / `fmt` stay on the native renderer
(reach for `(to-str x)` explicitly there). It rides on the prelude's `*show*`
dynamic var, so `(binding (*show* nil) …)` disables it for a scope. Default record
printing keeps its `:__id__` — that is intended (records print unlike bare maps); the
protocol is the per-record *override*, not a change to the default.

#### `defbehaviour`: the module-as-implementor contract (`require 'protocol`)

An ability dispatches on a **value**. When the unit that implements a contract is a
*module* — a live view the router calls by name, say — there is no value to
dispatch on, and that is `defbehaviour`'s job (`std/protocol.blsp`):

```clojure
;; where the contract is declared
(defmodule views (:use protocol))
(defbehaviour LiveModule
  (mount [params])
  (render [model])
  (handle-event [name params model]))

;; a module that satisfies it — the header claims it, the module defines the ops
(defmodule counter-view (:use views) (:implements LiveModule))
(defn mount (params) {:n 0})
(defn render (model) (str "count: " (get model :n)))
(defn handle-event (name params model) (assoc model :n (+ 1 (get model :n))))
```

`defbehaviour` declares the ops and defines **no functions of its own** — a module
satisfies the contract by defining them itself and calling them directly. A module
claims it with `(:implements Name)` in its header, and `nest check` verifies the
module defines each declared op at the right arity. `(protocol-ops 'Name)` exposes
the specs as data.

> **`defprotocol` / `defimpl` were retired** (ADR-168) in favour of `ability`, which
> subsumes them: they dispatched only on `type-of`, so no two records could ever
> dispatch apart. `defbehaviour` stays because a module contract is genuinely a
> different thing. Reaching for `defprotocol` now raises an unbound-symbol error
> whose hint points at `defability`/`impl`.

#### The abilities the standard library ships

Beyond the core `Display`/`Inspect`, `std/` declares these (ADR-177). Each is the
extension point for its module: `impl` it for your own type and that module accepts the
type with no change to it.

| Ability | Op | Module | Sealed? | What impl'ing it buys |
|---|---|---|---|---|
| `JsonEncode` | `(to-json x)` | `json` | open | `json-encode` accepts your type — a record picks its wire shape, and a kind JSON has no rule for (a pid, a datetime) stops erroring. No `:default`. |
| `Port` | `(io-write port s)` | `io` | open | Your value is an output port: `io-write`, `with-out`/`with-err`, and every logger backend take it. A bare 1-arg fn is a port via the `:fn` impl. |
| `LogBackend` | `(backend-emit b record)` | `log` | open | A backend that does something other than "format one line and write it" — batching, JSON lines, sampling. Reuse `backend-passes?` for the standard level/filter gate. |
| `Response` | `(send-response r sock)` | `net/http` | open | A response kind with its own wire behaviour (sendfile, chunked, a 101 upgrade), including whether it closes the socket. |
| `Dependency` | `dep-kind`, `dep-resolve`, `dep-check-compatible`, `dep-lock-vec`, `dep-entry-node` | `package` | **sealed** | A new manifest dependency kind. Sealed, so `nest check` reports any op you forget. |
| `Temporal` | `(to-iso x)` | `datetime` | **sealed** | ISO 8601 rendering for a calendar value. |

Sealed vs open is a per-ability judgement: `Dependency` and `Temporal` cover genuinely
closed sets and want exhaustiveness checking; the rest exist so a type the module has
never heard of can join.

`std/` also uses `defrecord` for the value types that were once plain maps told apart by
their shape — `buffer`, `queue`, `pq`, `multimap`, `datetime`/`date`/`time-of-day`, and the
four dependency kinds. Each has an identity-based predicate (`buffer?`, `queue?`, …) that a
look-alike map can't satisfy, and a `Display` impl, so it prints as itself rather than as
its internals. Where a library renders a *user-supplied* value into text —
`template/render`, `csv-emit`, `url/query-encode` — it goes through `to-str`, so your
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
(defmulti  num-add :commutative)                                  ; declare (with an algebra)
(defmethod num-add [money money] (a b) (money (+ (cents a) (cents b))))
(defmethod num-add [money :int]  (m n) (money (+ (cents m) n)))   ; scalar mixing, if you want it

(+ (money 100) (money 50))   ;=> (money 150)
(+ (money 100) 5)            ;=> (money 105)
(+ 5 (money 100))            ;=> (money 105)   ; commutative: the [:int money] mirror is derived
```

**Resolution is total and unambiguous by construction.** A call resolves by (1) an **exact**
tuple match, else (2) a single declared **`:default`** method, else (3) a loud, structured
`no-method` error (`{:kind :no-method :multi :key :have}`). There are **no partial wildcards**
and therefore **no ambiguity** — a key hits exactly one method or the default, never a
"nearest" guess. (Partial patterns + a specificity order are deliberately deferred; they are
where ambiguity lives, and Brood's flat record ids give no basis to break a tie — ADR-179.)

**Closure algebra — authoring a binary op as its upper triangle.** A `defmulti` may declare an
algebraic property that *derives* each off-diagonal method's mirror, so you write N(N+1)/2
methods instead of N² and never hit a diagonal ambiguity:

- **`:commutative`** (`+`, `*`, `min`, `max`) — registering `[A B]` also registers `[B A]` as
  `(f y x)`. Sound because `a⊕b = b⊕a`.
- **`:antisymmetric`** (`compare-to`) — the mirror is `(- 0 (f y x))`. Sound because
  `cmp(a,b) = −cmp(b,a)`.
- **none** (`-`, `/`) — no mirror; `(- 5 money)` is a `no-method`, because subtraction is
  genuinely asymmetric and the system won't fabricate a mirror the math doesn't license.

There is **no implicit coercion**: a closure derives the mirror of a method *you authored*, it
never invents a conversion — so cross-type arithmetic is explicit-per-method yet symmetric.

**`Num` and `Ord` are the built-in multimethods.** `+`/`-`/`*`/`/` route to
`num-add`/`num-sub`/`num-mul`/`num-div`, and `<`/`<=`/`>`/`>=`/`min`/`max` route to
`compare-to`, **only when a record is an operand** — pure `int`/`float`/`decimal` arithmetic
stays byte-for-byte on the kernel's fast path (records never touch it). Both are **strict**:
neither has a `:default`, so a record type must define the methods it means. `(+ (money 1)
2.5)` with no matching method is a hard `no-method`, and likewise `(< (money 1) 2.5)` — and
`(sort some-records)` — is a `no-method` until you give the record a `compare-to`. A record is
never silently ordered by its underlying map layout; define `compare-to`/`num-*` for the pairs
you actually mean.

**Strict declarations (fail at load, and `nest check`).** A `defmethod` whose arg count differs
from its pattern length, a closure on a non-binary pattern, an unknown algebra keyword, a
method for an undeclared `defmulti`, or `:default` used as a tuple position are all **hard
errors at load** — nothing unclear is silently accepted. Redefining a `defmethod` is ordinary
hot reload (a `def` into the `*methods*` registry), visible on the next dispatch.

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
work on one with no import: `(conj s x)` adds, `(disj s x)` removes,
`(contains? s x)` tests membership, `(get s x)` returns the *element* when present
(never a positional index) and `nil`/`default` when not, `(into #{} coll)` pours
and dedups, and `count`/`empty?`/`first`/`rest`/`vec`/`seq` treat it as its
elements. The kernel supplies the literal, `set?`, and the O(log n) element ops
(`%set`/`%set-add`/`%set-remove`/`%set-has?`/`%set-count`).

The **`set` library** (`(require 'set)` / `(:use set)`, `std/set.blsp`) adds only
what's specific to sets: the constructor from a collection (`(set coll)`, which
dedups) and the algebra `union`/`intersection`/`difference`/`subset?`. Sets
deep-copy across processes like any value (`send`/`spawn` round-trip them as sets,
not maps).

> Until 2026-07-26 `conj`/`disj` lived in `std/set` instead, and the prelude's own
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
eagerly). They are reserved names.

| Form | Meaning |
|---|---|
| `(quote x)` / `'x` | `x`, unevaluated. |
| `(if test then else?)` | Evaluate `then` if `test` is truthy, else `else` (or `nil`). |
| `(do body...)` | Evaluate forms in order; result is the last. |
| `(def name value)` | Define/redefine `name` in the **global** environment — redefinable, the language's only mutation. |
| `(fn (params) body...)` | A lexical closure. (`lambda` is still accepted as an exact synonym — ADR-108 — though `fn` is the only spelling used anywhere in the tree; retiring the alias is a pending cleanup, see ROADMAP.) |
| `(let (a 1 b 2) body...)` | Sequential local bindings (each sees the previous). Brood's `let` is already sequential, so there is no separate `let*`. |
| `(letrec (f (fn ...) g (fn ...)) body...)` | Local **mutually recursive** bindings — every name is visible in every RHS (and to itself). Plain-symbol targets only; meant for fn definitions. |
| `` (quasiquote tmpl) `` / `` `tmpl `` | Template: literal except `~x` inserts a value and `~@xs` splices a sequence. |
| `(defmacro name (params) body...)` | Define a macro (see below). |

`when`, `unless`, `cond`, `and`, `or`, `case`, `match`, and `comment` read like
special forms but are **prelude macros** over `if`/`do`/`let`
(`std/prelude.blsp`), expanded once by the compile pass (ADR-022) — so the
evaluator's core stays minimal and they cost nothing extra at runtime.
(`(comment body…)` ignores its body and yields `nil` — the form-level "don't run
this", since Brood has no `#_` discard reader macro. The body is still *read*, so
it must be balanced sexps; the checker skips it, so names inside need not resolve.) `cond` is still flat test/expr pairs with **`else`** as
the catch-all (ADR-004; `:else` was a second blessed spelling and is no longer
special — it still catches, but only because a keyword is truthy, exactly as
`true` or `42` would); `and`/`or` short-circuit left-to-right and return the
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

The `->`, `->>`, and `as->` threading macros are also defined in the prelude:

```clojure
(-> 5 (- 1) (* 2))            ;=> 8     ; (* (- 5 1) 2)   thread as FIRST arg
(->> (list 1 2 3) (map inc))  ;=> (2 3 4)                 thread as LAST arg
(as-> 5 $ (+ $ 1) (* $ 2))    ;=> 12    ; bind $, thread into ANY position
```

The **conditional / short-circuit threading** macros build on those, plus `doto`:

```clojure
(some-> {:a {:b 5}} (get :a) (get :b) inc)        ;=> 6      ; stop at the first nil step
(cond-> {} true (assoc :a 1) false (assoc :b 2))  ;=> {:a 1} ; apply a step only when its guard holds
(doto (table) (table-put :a 1) (table-put :b 2))  ; run forms for effect, return the value
```

`some->>`/`cond->>` are the thread-*last* variants; `run!` applies a function to
each item for effect (`(run! println xs)`, the function form of `doseq`).

**Binding-conditionals** bind, test the *source* value, then branch (the target may
destructure):

```clojure
(if-let (v (get m :k)) (use v) :absent)   ; bind v; take `then` when truthy, else `else`
(when-let (v (get m :k)) (use v))         ; body only when truthy
```

**`with`** — Elixir's `with`, spelled as flat `pattern expr` pairs (the `let`
shape). Each `expr` is matched against its `pattern` in order; the first value
that fails its pattern **short-circuits**, and the body runs only when every step
matched, with all bindings in scope. It's pure sugar over nested `match` (no new
special form):

```clojure
(with ([:ok account] (lookup user)
       [:ok card]    (payment-method account)
       [:ok receipt] (charge card 10))
  receipt)                                  ; a step's [:error …] falls straight through
```

A trailing **`:else`** section is a set of `match` clauses run against the value
that short-circuited (like Elixir's `else`); with no `:else`, that value is
returned as-is:

```clojure
(with ([:ok user] (lookup id))
  user
  :else
  ([:error :not-found] {:error "no such user"})
  ([:error e]          {:error e}))
```

**Local loops** — there is **no `loop`/`recur`**; Brood has proper tail calls, so a
self-contained loop is a `letrec`-bound closure you call by name (it closes over the
enclosing scope, so you thread only the *changing* state, and the tail call is O(1)
stack):

```clojure
(letrec (go (fn (n acc) (if (= n 0) acc (go (- n 1) (+ acc n)))))
  (go 100000 0))                                ;=> 5000050000, O(1) stack
```

**`fmt`** — string interpolation. `(fmt "…{expr}…")` splices each `{expr}` hole's
value between the literal text and lowers to a plain `(str …)` (no runtime cost);
`{{`/`}}` are literal braces and braces nest inside a hole:

```clojure
(fmt "sum={(+ a b)} for {name}")   ;=> "sum=7 for ada"
```

**`spy`** — a homoiconic tree-tracing debug macro (Brood's answer to Elixir's `dbg`,
ADR-173). `(spy expr)` evaluates `expr`, traces **every** evaluated subexpression's
value in evaluation order, and returns the value **unchanged** — so it's referentially
transparent: wrap or drop `spy` around any form with no behavioural change.

```clojure
(spy (+ (* a 2) (f 2)))     ; a=10, f adds 5
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

```clojure
(binding (*spy-sink* (fn (entry) nil)) (spy (heavy-computation)))  ; value, no output
```

> Note: a **nested quasiquote** (a `` ` `` inside a `` ` `` template) is
> **rejected**, with a hint. Levels are not tracked, so an inner `~x` would be
> expanded at the outer level — `` `(a `(b ~(+ 1 2))) `` used to yield
> `(a (quasiquote (b 3)))` where the standard reading leaves `(+ 1 2)`
> unevaluated. A `` ` `` inside an `~unquote` is ordinary code and stays legal, so
> the macro-writing-a-macro spelling is `` `(a ~(inner-template x)) ``. Level
> tracking can land later without breaking anything accepted today (ADR-011). Auto-gensym (`x#`) / `gensym`
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
| `^expr` | the current value of `expr` (a *pin*) |
| `(p1 p2 …)` | a list of that exact length, element-wise |
| `(p1 & rest)` | head(s) + the tail bound to `rest` |
| `[p1 p2 …]` | a vector of that exact length — the **tagged-data / tuple idiom** |
| `{:keys [a b] :or {a 0}}` | a **map** — binds each `:keys` symbol to the same-named keyword's value (nil if absent, or the `:or` default); fails if the value isn't a map. `{}` matches any map. |
| `(bytes seg…)` | a **`bytes` value**, destructured segment-by-segment — Erlang/Elixir bit syntax (see below) |
| `(or p q …)` | any alternative — first match wins; every alternative must bind the same names |
| `(and p q …)` | every pattern, against the same value — the capture-while-destructuring (`:as`) idiom |
| `{:k p}` | a map with key `:k` **present**, whose value matches `p` (nests to any depth) |

Patterns nest to any depth. **The one trap:** a bare symbol *binds* (and
shadows) — it does **not** test against a same-named value. Match a known value
with a keyword (`:ok`), a quoted symbol (`'none`), or a pin (`^x`).

### Alternatives, conjunctions, and map sub-patterns

`(or p q …)` matches if **any** alternative matches, first one wins:

```clojure
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

```clojure
(match msg
  ((and whole {:keys [kind]}) (log kind whole)))
```

A map pattern's **explicit keys are sub-patterns**: `{:status 200}` requires the key
to be *present* and its value to match, nesting to any depth
(`{:user {:id id}}`). Note the deliberate split, one convention from each ecosystem:

| Spelling | Semantics | If the key is absent |
|---|---|---|
| `{:keys [a b]}` | Clojure **destructuring** | binds `nil` (or the `:or` default); never fails |
| `{:k pat}` | Erlang/Elixir **map pattern** | the clause **fails** |

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

A `(bytes seg…)` pattern destructures a `bytes` value left-to-right, Erlang
bit-syntax style. Each segment consumes bytes:

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

```clojure
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
parsing: `(bytes-uint bs off n)` / `bytes-uint-le` / `bytes-int` /
`bytes-int-le` (n = 1–8 bytes), and the encoders `(int->bytes v n)` /
`int->bytes-le` (truncating to `n` bytes, the bit-syntax convention — so
`(int->bytes -1 2)` is `#b"\xff\xff"`).

### `match`

Clauses are **wrapped** `(pattern [:when guard] body…)`; the first whose pattern
(and guard) matches runs its body.

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

### `case` — literal dispatch

`case` dispatches on a **value** against literal tests, written as flat
`test result` pairs (`cond`'s shape) with a lone trailing form as the default:

```clojure
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
`[:match-error :case value patterns]`, exactly as `match` does, and the
[exhaustiveness lint](#type-annotations) covers a `case` over a declared literal
type just as it covers a `match`.

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
  ((a & more) (+ 1 (count more))))

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
- **`&optional`/`&` in a pattern-dispatched `defn` is an error.** A multi-clause
  `defn` is *either* arity-dispatched (every head is plain symbols, optionally
  with `&`/`&optional`) *or* pattern-dispatched (some head carries a
  literal/destructuring form). If any head is a pattern, an `&optional`/`&`
  marker in *any* head is rejected with a hint — it used to be matched as a
  literal symbol, so the clause silently stopped being variadic and a call like
  `(f 1 2)` failed with a `[:match-error …]` listing `(x &optional (y 5))` as a
  *pattern*. Use one mechanism per `defn`.
- **Overlapping arity arms resolve most-specific-first**, in this order: an exact
  fixed arity beats a variadic (`&` rest) one; then the most required params;
  then the *fewest* `&optional` slots. So with `((x) :one)` and
  `((x &optional y) …)`, `(f 1)` picks `:one` regardless of clause order. (Before
  the last tie-break the answer depended on the order the clauses were written.)
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

`catch` takes **exactly one bare binder** and no exception class — Clojure's
`(catch Type e body…)` is rejected with a hint, since reading it Brood's way would
bind the *class name* and evaluate the intended binder as a statement (a wrong
program, not an error). `catch` binds `e` to the thrown value: a `throw` hands back its argument verbatim
(a bare string from `error`, a keyword, a `[:tag …]` vector, …), while a built-in
error (like division by zero) binds the kernel's canonical **error map** —
`{:kind :message [:code :file :line :col :hint :trace]}` — so a handler can
branch on `(get e :kind)` without parsing strings. A `try` with no `catch` is
just a `do`. Under the hood `throw` and `%try` are primitives and
`try`/`catch`/`error` are written in Brood (`std/prelude.blsp`) — see
[primitives.md](primitives.md).

**`:trace` is the call stack at the raise** — a list of frames, innermost first,
each a `{:fn <name> [:file <file> :line <l> :col <c>]}` map whose location is the
**call site that entered the frame** (absent fields are omitted; anonymous frames
have no `:fn`). It covers the frames between the raise and the `catch` that
caught it, capped at 32 (deep recursion keeps the innermost frames — the end
that shows the cycle). Proper tail calls collapse into their caller's frame, so
a tail chain `outer → middle → boom` shows one frame named for where the chain
ended — the Erlang behaviour, and a direct picture of the real (O(1)-stack)
frame structure. Uncaught errors print it as `at fn (file:line:col)` lines under
the diagnostic; it costs nothing on the non-throwing path. A user `(throw v)`
carries `v` verbatim (no map, so no `:trace` on the caught value).

Because a caught value has no single shape, **`(error-message e)`** is the
shape-agnostic accessor: a raised string as-is, the `:message` of an error map,
else the value's printed form. A `catch` handler that just wants a human string
uses it directly instead of branching on `string?`/`map?`:

```clojure
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

(level)                          ; => 0   (*indent* is its default)
(binding (*indent* 4) (level))   ; => 4   (rebound for this dynamic extent)
(level)                          ; => 0   (restored afterwards)
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

`std/io.blsp` gives the port toolkit — constructors and the `with-out`/`with-err`
scoping macros (thin wrappers over `binding`). Pull it in with `(:use io)` so the
names read bare (a bare `(require 'io)` only *loads* it — you would then write
`io/fn-port`):

```lisp
(defmodule my-app (:use io))

(with-out (fn-port (fn (s) (collect s)))   ; route output to a callback
  (println "captured by collect"))

(with-out (process-port editor)            ; route output to another process …
  (println "sent as [:io-write \"…\\n\"]"))
```

A **`process-port`** sends each string to a process as `[:io-write s]`. That is
how output reaches a *buffer*: the process that owns the buffer (an editor's
`*Messages*`, say) receives the message and appends it. The string crosses the
process boundary as a copied message — async and share-nothing, never a mutated
value — which is exactly why it's safe. (Dynamic bindings don't
reach a `spawn`ed child, so a child starts with the default `*out*`; hand it a
port explicitly if it should redirect too.)

A port is any value implementing the **`Port`** ability, whose one op is `io-write` — so
a bare 1-arg fn is a port (that is the `:fn` impl), and so is a port *record* that
carries its target and prints as itself (`#<port stdout>`, `#<port file /tmp/app.log>`).
`port?` tests it, and your own type joins with `defrecord` + `impl Port`. `*out*`/`*err*`
still hold a plain fn, which the prelude's `print` calls directly — so printing pays no
dispatch cost, `print` gains no special cases, and `with-out-str` is unaffected;
`with-out`/`with-err` adapt a record port at that boundary for you (`port-fn`).

### Logging

`std/log.blsp` is an **async, safe logger** built on the same idea. A logger is
one long-lived process (a `proc/gen` server) holding a list of *backends*; each
log call is a fire-and-forget cast, so it never blocks the caller, and the single
process serialises every write — lines never interleave, and a backend that throws
takes down only that line, not the caller.

```lisp
(defmodule my-app (:use log))

(start-logger)                          ; default: stdout, :info and up
(log-info "server up" {:port 8080})     ; structured fields are optional
(log :warn "disk low")
;; => [INFO  1736…] server up
;;    [WARN  1736…] disk low
```

Levels are `:debug` < `:info` < `:warn` < `:error`. A **backend** is any value
implementing the **`LogBackend`** ability (one op, `backend-emit`, handed each record).
The stock one is an `io` port + a minimum level + a filter + a formatter, so the logger
*reuses* ports rather than inventing its own sink; build it with `stdout-backend` /
`stderr-backend` / `file-backend` / `fn-backend` / `process-backend`, and add it live:

```lisp
(add-backend (file-backend "app.log"))         ; also append to a file
(add-backend (process-backend buffer-pid))     ; …and to a buffer-owning process
```

`process-backend` is the **log-to-a-buffer** path: the formatted line is sent to
`buffer-pid` as `[:io-write s]` — the same envelope `process-port` uses — so an
editor process can fold it into its `*Messages*` buffer. The default logger is
registered under the name `:logger` (found via `whereis`); `(log …)` falls back to
stderr when none is running, so a log is never silently lost.

For a backend that does something other than write one formatted line — batch records,
emit JSON lines, sample — `defrecord` your own and `impl LogBackend` for it; the logger
takes it unchanged, and `backend-passes?` gives you the same `:min-level`/`:filter` gate
every stock backend honours.

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

The type grammar: base names — `int float number decimal string symbol keyword
bool nil pair vector list map set bytes fn rope pid ref table socket subprocess`,
plus `any` (everything) and `never` (nothing); the spellings match what `type-of`
returns, with `number` = int∪float, `list` = nil∪pair, and `fn` = closure∪native.
Then function arrows `(p… -> r)`, element-typed
sequences `(list E)` / `(vector E)`, unions `(or A B …)` and intersections
`(and A B …)`, literal (singleton) types — a bare keyword/int/bool/string
(`:foo`/`5`/`true`/`"GET"`) — any combination composing freely in one `(or …)`
(see [type-int-literals.md](type-int-literals.md) and
[type-bool-string-literals.md](type-bool-string-literals.md)), type
variables (`?A`), key/value-typed maps `(map K V)` (see
[type-map-kv.md](type-map-kv.md)), and heterogeneous record shapes `(record
:k1 T1 :k2 T2 …)` with required-by-default fields and an `(optional T)` wrapper
for optional ones (see [type-records.md](type-records.md)). An unrecognised
type-expression is ignored, never guessed.

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
run time*: it wraps `name` so each argument and the result are checked on every
call, throwing on a mismatch (an opt-in "strong arrow"). Place it **after** the
definition — it rebinds the name, preserving arity.

```clojure
(defn area (r) (* 3.14159 r r))
(sig! area (number -> number))
(area "circle")   ;=> throws — area: argument 1 expected number, got string
```

`sig` is checker-only (zero runtime cost); `sig!` adds the runtime guarantee
exactly where you want soundness.

**Placement: put a `sig` *below* its definition.** As a declaration it works
anywhere, but `BROOD_CONTRACTS=1` makes every `sig` behave like `sig!` — which
*rebinds* the name — so a `sig` above its `defn` fails under that flag (it now says
so, naming the fix, instead of dying with `unbound symbol`). `std/` follows the
below-the-definition rule, and `tests/sig_adoption_test.blsp` checks it
structurally. A corollary: **prelude functions can't carry a `sig`** — a runtime
contract wraps the function in a closure that captures a local frame, and the
prelude freeze requires shared closures to capture only the global environment.

Adoption started in `std/path`, `std/json`, and `std/set` (ADR-153); the checker
enforces those declarations at every call site, in any module, and result types
flow (a `bool` result handed to `string-length` is caught). The checker treats both identically. Writing a
*type* never changes behaviour; opting into *enforcement* (`sig!`) does. Full
design: [type-annotations.md](type-annotations.md) (ADR-082).

### Advisory lints (non-type warnings)

`nest check` / `brood --check` emit several additional warnings beyond type
misuse — all advisory, zero false positives:

**Unused `let` bindings** — a name bound in `let`/`letrec` that never
appears in its visible scope (subsequent binding RHSs + body). Names prefixed
with `_` are exempt (intentional don't-care). Compiler-generated `let`s from
match/pattern expansion are also exempt.

```clojure
(let (x 1 y 2) x)   ; warning: unused let binding: y
(let (_y 1) 2)       ; silent — _-prefix means intentional
```

**Unused `:use` imports** — a `(:use mod)` clause whose contributed public names
are never referenced in the file. Only fires when the module contributed at least
one name (so a failed `require` or an empty module is silent).

```clojure
(defmodule my/app (:use io) (:use json))
(defn handler (x) (json-encode x))
; warning: unused :use import: io — io-write, stdout-port, etc. never used
```

**Unused module-private defns** — a `defn` whose bare name contains `--` (the
private-by-convention marker, same gate as `(:use …)` refer-all skipping) but
which is never referenced anywhere in the project — neither as a same-module
unqualified call nor a cross-module / test `mod/name` reference. Checked at the
*whole-project* layer (`nest check`), not by a single-file check: a `--` name is
a convention, not enforced privacy, so it is legitimately reached from another
module or a test by its qualified name, which a per-file scan can't see. Public
names are never checked.

```clojure
(defmodule my/mod)
(defn helper--parse (s) …)   ; warning: unused private function: helper--parse
(defn run (s) s)              ; (helper--parse is defined but never called)
```

All three lints share the "zero false positives" contract: they are conservative
(count any occurrence, including in binder positions, as "used") and emit nothing
when static information is ambiguous.

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
form. The primitive only *selects* a clause (answering which one matched and what
its pattern bound); every clause **body is emitted at the call site**, so bodies
compile into the enclosing function's own code and a receive loop's tail call is
an ordinary tail call in that function (ADR-155). See
[concurrency.md](concurrency.md) and [scheduler.md](scheduler.md) for the model,
and [pattern-matching.md](pattern-matching.md) for the clause grammar.

### Synchronous calls (and why there's no `await`)

`send` is fire-and-forget. To wait for a result, you don't need an `await`
primitive — the **blocking `receive` is the synchronisation**. The idiom is
Erlang's `gen_server` distinction: a *cast* is a bare `send`; a *call* is a
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

`std/proc/gen.blsp` packages the request/reply idiom above into a
gen_server-style framework — ~180 lines of Brood over `spawn`/`send`/`receive`/
`ref`/`monitor`, no kernel surface (ADR-099). A server carries one immutable
state value through a tail-recursive `receive` loop; `defprocess` declares how it
handles each kind of message. Pull it in with `(:use proc/gen)` so `defprocess`,
`spawn-server` and `!` read bare (a bare `(require 'proc/gen)` only loads it):

```clojure
(defmodule my-app (:use proc/gen))

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
`proc/supervisor` (see `std/proc/supervisor.blsp`).

### Monitors

`(monitor pid)` starts watching another process and returns a monitor `ref`.
When that process dies, the watcher receives one message:

```clojure
[:down <monitor-ref> <pid> <reason>]
```

`reason` is `:normal` for a clean return, `[:error <error-map>]` for a crash —
the same structured `{:kind :message [:code :file :line :col :hint :trace]}` map
a `catch` binds (call `:trace` included — BEAM's `{Reason, Stacktrace}`), so a
supervisor can log `(get m :message)` and walk `(get m :trace)` from the reason
alone — and
`:noproc` if `pid` was *already* dead when you called `monitor` (the DOWN is then
delivered immediately). The monitor is **unidirectional** (it never affects the
watched process) and **one-shot** (it fires once). `(demonitor mref)` drops it,
best-effort — a DOWN already queued is not recalled. Pin the ref to wait for a
specific process's death and ignore unrelated messages:

```clojure
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
(Erlang `link/1`; `(unlink pid)` unties, `spawn-link` spawns pre-linked). When
either side dies abnormally, the other is notified: a process that set
`(trap-exit true)` receives a trappable `[:EXIT pid reason]` message; a
non-trapping process **dies too** — propagation, cascading through *its* links
in turn. A `:normal` exit never kills a non-trapping peer. The propagated death
carries the **originating reason**: if `a` crashes with `[:error {…}]`, a linked
non-trapping `b` dies with that same reason (and so does `c` linked to `b`), so
monitors anywhere in the fallen tree report the root cause — not a blanket
`:kill`. Links are what `proc/supervisor`'s trapping supervisor loop is built
on; remote (cross-node) links deliver the same shapes, plus `:noconnection` on
a net-split.

### Timers

`(send-after ms pid msg)` delivers `msg` to `pid` after `ms` milliseconds
(Erlang `send_after/3`, same argument order); `(send-interval ms pid msg)`
delivers it every `ms` until cancelled. Both return a **timer handle** (the
timer's pid — a tiny green process parked on the scheduler's timer wheel, so a
pending timer costs no worker thread); `(cancel-timer h)` stops it, idempotently
(and, like Erlang, cancellation races an in-flight fire — a message already sent
stays sent). An interval timer monitors its target and exits when the target
dies, so a forgotten interval can't tick forever.

```clojure
(def t (send-interval 1000 (self) :heartbeat))
(receive (:heartbeat (redraw)))
(cancel-timer t)
```

### Blocking natives (`offload`)

A long or blocking native — a git clone, a key derivation, big file IO — would
pin its scheduler worker for the whole call. `(offload f & args)` runs it on
the **dirty-offload OS pool** instead (the BEAM dirty-scheduler shape,
ADR-144): only the calling *process* waits (a selective receive under the
hood, so other mailbox messages stay queued), and errors rethrow at the call
site, catchable as usual.

```clojure
(offload slurp-bytes "big-archive.tar")     ; the worker keeps running others
(offload %pbkdf2-sha256-bytes pw salt 600000 32)
```

Only long/blocking **data-in/data-out** natives are allowed (`%git-clone`,
`%git-resolve-ref`, `%pbkdf2-sha256-bytes`, `%digest`, `%hmac`, `slurp`,
`slurp-bytes`, `spit`, `spit-bytes`, `spit-append`, `append-bytes`,
`tls-self-signed`); anything heap-sharing or env-reading is refused with a
clear error. Args and the result are deep-copied across (like `send`), so
they must be sendable values. The package manager's clones already ride it.

### Per-process limits (`process-flag`)

`(process-flag flag [value])` reads or sets a runtime flag on the **current**
process (Erlang's `process_flag/2` shape) and returns the previous — or, with no
value, current — setting. The first flag is **`:max-heap`**: a per-process heap
limit in bytes, the BEAM `max_heap_size` analogue.

```clojure
(process-flag :max-heap 8000000)   ; cap this process at ~8 MB; returns previous
(process-flag :max-heap)           ; read it
(process-flag :max-heap nil)       ; clear it (also cancels a pending trip)
```

The limit is checked after each of the process's own GC collections against the
**live** (post-collection) footprint, so transient garbage never trips it. When
exceeded, the next safepoint raises a catchable error (`E0045`) **in that
process only** — uncaught, it kills just the offender, and every other process
(and the runtime) is untouched. That's the isolation the global
`BROOD_MEM_LIMIT` cap can't give: its hard tier aborts the whole OS process.

Policy stays in Brood: to spawn a capped worker, set the flag first thing in
the spawned fn —

```clojure
(spawn (fn () (process-flag :max-heap 8000000) (work)))
```

### Going idle (`hibernate`)

`(hibernate)` tells the runtime this process is about to wait a long time, so it
should give back everything it can: collect, shrink its heap, and drop its
inline caches and compiled-body cache. It returns the bytes released. This is
Erlang's `erlang:hibernate/3` (without the continuation argument — a Brood
process resumes from its own `receive`).

```clojure
;; a pooled connection that will sit idle between requests
(defn serve (conn)
  (hibernate)                       ; ~40% smaller while parked
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

The cookie is a shared secret (Erlang-style; links are encrypted — ADR-089). One
node per OS process. Remote `spawn`/code-shipping, distributed monitors/links,
heartbeat node-down detection, and mesh join have all shipped — full reference:
[distribution.md](distribution.md).

**Send semantics across a net-split:** `send` to a *disconnected* node silently
drops the message (Erlang's default). A process that must not lose messages opts
in with `(process-flag :send-errors true)` — its sends then raise a catchable
`E0060` noconnection error, so it can queue and resend. Pair it with
**`net/reconnect`** (`(require 'net/reconnect)`): `(net/reconnect/watch spec)`
keeps the link alive with exponential-backoff reconnects, and
`(net/reconnect/subscribe spec)` delivers `[:nodedown name]` / `[:nodeup name]`
to your mailbox — resend the queue on `[:nodeup …]`.

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
- **Decimal arithmetic preserves scale** — `+`, `-` and `*` on exact operands
  (decimal/int/bignum) give the result the standard's *ideal exponent*: the finer
  of the two scales for `+`/`-`, the sum of them for `*`. So `1.50M * 2.25M` is
  `3.3750M`, not `3.375M`, and `(- 1M 0.0M)` is `1.0M`, not `1M` — significance
  survives the operation, which is the point of using a decimal for money. Only a
  zero *result* prints scale-less (`(- 1.50M 1.50M)` renders `0`). A float operand
  anywhere makes the result an inexact float (contagion), and `/` is inexact by
  nature, so neither carries an ideal exponent. Pinned by the dectest conformance
  corpus (`tests/conformance_dectest_test.blsp`).
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

### Float bit patterns
`float->bits`  `bits->float`

- `(float->bits x)` is the IEEE 754 binary64 bit pattern of `x` as a non-negative
  integer — a bignum whenever the sign bit is set, since the pattern is a *u64*.
  `(bits->float n)` is the inverse, for `n` in `[0, 2^64)`.
- This is **reinterpretation, not conversion**, and it is the only *exact* float
  comparison the language has. `=` on floats is value equality, which deliberately
  collapses `-0.0` and `0.0` and reports every NaN as equal to nothing:

  ```lisp
  (= -0.0 0.0)                                  ; => true
  (= (float->bits -0.0) (float->bits 0.0))      ; => false
  ```

- An `int` argument is taken as its float value, so `(float->bits 1)` and
  `(float->bits 1.0)` agree.
- Rust primitives: no bitcast or `frexp` exists to bootstrap them from. They are
  what the `parse-number-fxx` conformance corpus asserts against
  (`tests/conformance_parse_number_test.blsp`).

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
`cons`  `first`  `rest`  `second`  `third`  `last`  `but-last`
`list`  `vector`  `vec`  `conj`  `disj`  `into`  `seq`  `enumerate`
`append`  `reverse`  `reverse-onto`  `nth`  `count`  `empty?`
`range`  `take`  `drop`  `split-at`  `take-last`  `drop-last`  `take-while`  `drop-while`
`member?`  `any?`  `every?`  `find`  `index-of`  `index-where`  `zip`
`partition`  `sort`  `sort-by`  `subvec`  `remove`  `remove-nth`  `keep`
`distinct`  `dedupe`  `group-by`  `flatten`  `interpose`  `interleave`
`repeat`  `repeatedly`

- `first`/`rest` of `nil` are `nil`. `nth` takes an optional default:
  `(nth coll i default)`.
- **One sequence view, every collection.** `first`/`rest`/`last`/`count`/`empty?`/
  `map`/`filter`/`fold`/`reduce`/`into`/`vec`/`seq` accept a list, vector, `bytes`,
  **set** (as its elements) or **map** (as its `[k v]` pairs) — so
  `(first {:a 1})` is `[:a 1]`, not an error, as it was before 2026-07-26.
  `(seq coll)` is the explicit coercion to that list view, and `(vec coll)` /
  `(into [] coll)` the vector form. `conj`/`into` insert at each kind's natural
  point and **preserve the kind** (vector→vector, set→set, map→map, list→list).
- `enumerate` pairs each item with its index as `[i x]` vectors — the
  `map-indexed`/`keep-indexed` idiom is `(map (fn ([i x]) …) (enumerate xs))`.
- `append` concatenates any number of sequences — lists *and* vectors, read as
  sequences — left to right, returning a **list**; wrap in `(into [] …)` for a
  vector. (The `concat` alias was removed — one spelling each.)
- `reverse-onto` is `(append (reverse xs) ys)` in **one** pass instead of four, and
  is the spelling to reach for in a tail-recursive loop that has accumulated a
  reversed prefix and wants to splice it back in front of the remainder — the shape
  every non-tail-to-tail rewrite lands in. `ys` is shared, not copied, so only the
  prefix costs anything.
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
- `any?`/`every?` return booleans (`every?` is vacuously true on the empty
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
  vectors, stopping at the shorter. `zip-with` combines two sequences element-wise via a
  binary function. `partition` chunks into `n`-sized groups, dropping a trailing partial
  chunk; `chunk-every` keeps the remainder. `chunk-by` partitions consecutive equal-key runs.
- `scan` is a running fold — returns a list of all intermediate accumulator
  values starting with the initial value (like Haskell's `scanl`).
- `mapcat` maps a list-valued function and concatenates the results. `min-by`/`max-by`
  select the extremum of a collection by a key function. `(clamp x lo hi)` constrains a
  number to the closed range `[lo, hi]`.
- `repeat` builds a list of `n` copies of a value; `repeatedly` calls a
  zero-argument function `n` times and collects the results.
- `sort` orders ascending (or with a strict less-than predicate:
  `(sort > xs)`); `sort-by` orders by a key function. Both are a **stable**
  merge sort. All of these are tail-recursive (stack-safe on long inputs).
- **Lazy, fusing pipelines.** `map`/`filter`/`keep`/`remove` are **eager** — they
  return a concrete list and run their function immediately (so `(map f xs)` for
  side effects works). When you want a pipeline to **fuse** — fold/reduce in a
  single pass with no intermediate lists — use the lazy combinators `lmap`,
  `lfilter`, `lkeep`, `lremove`, threaded with `->>`:
  `(->> (range n) (lfilter odd?) (lmap sq) (reduce + 0))`. Each returns a **lazy
  seq-view** — an O(1) value (like a [lazy range](#lists--sequences)) that stands
  in for the list it would produce. Chaining composes the stages onto one view,
  so the whole pipeline walks the source once, building nothing in between (≈3×
  faster than the eager form on large inputs). `(seqview? x)` tests for an
  unrealised view; consume one with `fold`/`reduce`/`sum`/`count`/`into`/`join`/
  `seq`/`first`, or realise it with `(seq v)` / `(into [] v)`. Two properties:
  like any lazy value a view defers its work (and any `throw` in its functions)
  until realised — **don't build a view for side effects**; use eager `map` or
  `doseq` for that — and a view is **heap-local**, so `send` refuses to ship one
  (realise it first) — it can't cross a process boundary or the network as a
  live view.

### Transducers
`transduce`  `xmap`  `xfilter`  `xremove`  `xkeep`

The `l*` combinators above are the ergonomic front end; `transduce` is the same
machinery with the stages exposed, for when the pipeline is **computed, reused, or
your own**:

```clojure
(transduce (comp (xfilter odd?) (xmap sq)) + 0 (range 10))   ;=> 165, one pass
(transduce (xmap inc) conj [] (list 1 2 3))                  ;=> [2 3 4]
```

A **transducer** is a function `(rf) -> rf'`, where a **reducing function** `rf` is
`(acc x) -> acc`; a stage wraps `rf` and may call it zero, one, or many times per
input. So a stage of your own is a plain `fn` — no protocol to implement:

```clojure
(defn xtake-while (pred)
  (fn (rf) (fn (acc x) (if (pred x) (rf acc x) acc))))

(transduce (xtake-while (fn (n) (< n 3))) conj [] (range 6))  ;=> [0 1 2]
```

Stages compose **left to right in data-flow order** under `comp` — the reverse of
ordinary function composition — because each stage wraps the *next* one's reducer.
`(comp (xfilter p) (xmap f))` filters, then maps.

### Maps
`hash-map`  `get`  `assoc`  `dissoc`  `contains?`  `keys`  `vals`  `reduce-kv`
`merge`  `merge-with`  `update`  `update-vals`  `update-keys`  `select-keys`
`zipmap`  `get-in`  `assoc-in`  `dissoc-in`  `update-in`  `map?`

See the [Maps](#maps) section above. `{ }` is the literal form; the rest are
immutable operations that return fresh maps. `count`/`empty?` work on maps too,
in **O(1)** — the CHAMP root node tracks its size (exposed by the `map-count`
kernel primitive), so neither walks nor materialises the entries.

### Higher-order
`map`  `filter`  `mapv`  `filterv`  `reduce`  `fold`  `apply`
`comp`  `partial`  `complement`  `constantly`  `identity`

```clojure
(map inc (list 1 2 3))        ;=> (2 3 4)
(filter positive? (list -1 2 -3 4)) ;=> (2 4)
(mapv inc (list 1 2 3))       ;=> [2 3 4]   (vector result)
(filterv even? (range 5))     ;=> [0 2 4]   (vector result)
(reduce + 0 (list 1 2 3 4))   ;=> 10
(apply + (list 1 2 3))        ;=> 6
```

`map`/`filter` return lists; `mapv`/`filterv` are the vector-returning variants
for when the caller needs indexed access — the named form of
`(into [] (map …))`.

`reduce` takes `(reduce f init coll)` or `(reduce f coll)` (first item as the
seed); `fold` is the strict 3-argument form it wraps (`(fold f init coll)`) and is
what the prelude itself folds with. Both are public and both stay: `reduce` is the
surface you reach for, `fold` the strict-arity primitive it dispatches to (ADR-163).

The function combinators build the callbacks those ops take:

```clojure
(map (partial + 10) (list 1 2 3))     ;=> (11 12 13)  ; fix the leading args
(filter (complement odd?) (range 5))  ;=> (0 2 4)     ; negate a predicate
(map (constantly :x) (list 1 2))      ;=> (:x :x)     ; ignore the argument
((comp inc (partial * 2)) 5)          ;=> 11          ; right-to-left composition
```

### Predicates
`nil?`  `pair?`  `list?`  `symbol?`  `keyword?`  `string?`  `number?`  `int?`
`float?`  `decimal?`  `bool?`  `fn?`  `vector?`  `map?`  `set?`  `ref?`  `range?`
`pid?`  `table?`  `bytes?`  `rope?`  `nan?`  `infinite?`

- `nan?` / `infinite?` classify the non-finite floats. They earn their place
  because `nan` and `inf` are *reader literals* (a bare `nan`/`inf`/`-inf` token is
  a float), so the language could produce them long before it could test for one:
  `=` reports NaN as equal to nothing, which is both the IEEE rule and the only
  way to detect it. `(nan? x)` says so by name.

- `(type-of x)` returns the runtime type tag as a keyword — `:int` `:float`
  `:decimal` `:string` `:symbol` `:keyword` `:bool` `:nil` `:pair` `:vector`
  `:map` `:fn` `:macro` `:native` `:ref` `:pid` `:table` `:bytes` `:rope`
  `:socket` `:subprocess` — the spellings mirror the predicates above (the last
  two are opaque handles with no dedicated predicate). A lazy `range` reports
  `:pair`, since it stands for the list it produces. It's the reflective
  primitive that in-language type checks build on; the predicates are the
  common-case shortcuts.

### Strings
`str`  `pr-str`  `string-length`  `substring`  `char-at`  `string->list`
`list->string`  `string->codepoints`  `codepoints->string`  `upper`  `lower`
`string->number`  `number->string`  `index-of`  `includes?`  `join`
`string-split`  `replace`  `trim`  `triml`  `trimr`  `blank?`  `starts-with?`
`ends-with?`  `string-repeat`  `pad-left`  `pad-right`  `to-fixed`  `format`
`string->graphemes`  `grapheme-count`  `grapheme-at`  `substring-graphemes`
`string-normalize`  `display-width`

- `str` concatenates the *display* form of its args; `pr-str` returns the
  *readable* form of one value.
- There is **no distinct character type** (deferred): a "character" is just a
  1-char string, so `(char-at s i)` and the elements of `(string->list s)` are
  strings. All indices are **char-based**, matching `string-length` (so they are
  correct for multi-byte UTF-8, not byte offsets).
- `substring`, `char-at`, `string-length` are the char-indexed accessors;
  `string->list` / `list->string` bridge to and from a list of chars.
- `string->codepoints` gives the chars as a **vector of integer codepoints** in
  one O(n) native pass — the random-access form text parsers index with `nth`
  and compare as ints (`std/regex`/`std/json`/`std/encoding` all scan it);
  `codepoints->string` is its inverse.
- **A code point is not a character — a grapheme cluster is.**
  `string->graphemes` gives the extended grapheme clusters (UAX #29) as a vector of
  strings: `"e"` + U+0301 is *two* code points but *one* cluster, and a flag emoji is
  four code points and one cluster. This is the unit to step a cursor by; stepping by
  code point splits a cluster and corrupts the text. `(apply str (string->graphemes
  s))` is `s`. `display-width` counts terminal cells over the same clusters (a CJK
  char or emoji is 2, a combining mark 0).
- **The cluster-indexed accessors** are `grapheme-count`, `(grapheme-at s i
  [default])`, and `(substring-graphemes s start [end])` — the grapheme-indexed
  counterparts of `string-length`, `char-at`, and `substring`, which are all
  *code-point*-indexed. Reach for these whenever an index is a cursor position:
  `(substring s 1 2)` can slice a cluster in half (leaving a bare `e` and an orphan
  combining mark) where `(substring-graphemes s 1 2)` keeps it whole. Out-of-range
  reads yield the default and ranges clamp, exactly like `nth`/`take`. They exist so
  the correct spelling is also the fast one: `(nth (string->graphemes s) i)` was the
  only way to read one cluster, and it materialised every cluster in the string on
  every keystroke (ADR-159).
- **`=` is byte-structural, so text that reads identically can compare unequal**:
  `"é"` is U+00E9 *or* U+0065 U+0301. `(string-normalize s form)` normalises, with
  `form` one of `:nfc` `:nfd` `:nfkc` `:nfkd`. Canonical (`:nfc`/`:nfd`) preserves
  meaning; compatibility (`:nfkc`/`:nfkd`) also folds presentation — `"ﬁ"` → `"fi"`,
  `"²"` → `"2"` — which is what you want for search and identifier matching and not
  what you want for round-tripping text. Both are pinned by the UCD conformance
  corpora (`tests/conformance_ucd_test.blsp`).
- `upper` / `lower` case-fold (Unicode-aware: `(upper "ß")` → `"SS"`).
- `string->number` is a **strict** parse — int if it is one, else float, else
  `nil`; it rejects partial input (`(string->number "3abc")` → `nil`) and
  surrounding whitespace (`trim` first if needed). `number->string` is its inverse
  (just `str` on a number).
- `index-of` returns the first char index of a substring or `-1`;
  `includes?` is the boolean form. `join` puts a separator between strings;
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
- `fmt` is **string interpolation** (a macro): `(fmt "x = {x}, y = {(to-fixed y 2)}")`
  splices each `{expr}` hole's value between the literal text, lowering to a plain
  `(str …)` — zero runtime cost, so it is just a terser `str`. `{{`/`}}` are literal
  braces; braces nest inside a hole (`(fmt "m={ {:a 1} }")`). Prefer it over
  quote-chopped `str` wherever text interleaves values, including `error` messages:
  `(error (fmt "index out of range: {i}"))`.
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
`to-fixed` (float formatting), and the O(n) char-access mechanisms
(`string-split`, `string->codepoints`, `string-span`/`string-span-until`,
`%str-index-of` — char indexing into UTF-8 is O(index), so a pure-Brood scan is
unavoidably O(n²)) are Rust primitives; the rest of the library is Brood over
`substring`/`str` (`std/prelude.blsp`) — the "write the language in the
language" principle.

### I/O
`print`  `println`  `eprint`  `eprintln`  `with-out-str`  `with-err-str`

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
- `(with-err-str body...)` is the stderr counterpart, and works differently
  because stderr does: `eprint`/`eprintln` write through the **`*err*` port**, so
  this rebinds that port to a collecting sink rather than using the kernel capture
  buffer. Two consequences follow — it captures only what goes through `*err*` (a
  diagnostic the *kernel* writes, e.g. `[reload] arity changed`, is not
  interceptable this way; that one has its own switch, `*reload-diagnostics*`), and
  it does not follow into spawned processes the way `with-out-str` does. Use it to
  **assert on a warning**, and to keep an expected warning out of a test run's
  output.
- For simple raw-terminal control, `(:use editor/ansi)` provides escape *strings*
  to `print`: `ansi-clear` (erase + home — the per-frame reset), `ansi-cursor`,
  `ansi-home`, `ansi-hide-cursor`/`ansi-show-cursor`. The ESC byte is the `\e`
  string escape. For a structured render-op frame buffer instead, use
  `std/display` (`term-draw`/`term-emit`).

### Iolists (write-boundary trees)

The byte-producing write boundaries — `tcp-send`, `proc-send`, `spit`,
`spit-append`, `spit-bytes`, `append-bytes`, and the in-memory materialiser
`bytes-concat` — accept any **iolist** (ADR-139, the Erlang/Elixir model): a
**string**, a **`bytes`** value, a **byte int 0–255**, or an arbitrarily nested
**list/vector** of iolists (`nil` is empty; an improper tail is a final leaf).
Describe the output as a tree and nothing is copied until the single flatten at
the write:

```lisp
(tcp-send sock [status-line headers "\r\n\r\n" body])   ; no (str …) accumulation
(bytes-concat ["ab" ["cd" nil "e"] (bytes 102) 103])    ; => #b"abcdefg"
```

String leaves are UTF-8 at text boundaries; a **binary**-mode socket/child
(`tcp-set-binary`) keeps its byte-string rule — each string leaf's codepoints
must be 0–255. Anything else as a leaf (a float, a keyword, an int > 255) is a
type error. This deletes the O(n²) `(str acc chunk)` accumulation class:
collect parts in a list and hand the tree to the boundary. `str`/`join` are
**not** iolist-aware — they render display forms (a list argument prints as a
list); use `bytes-concat` (or `utf8-bytes->string` of it) to materialise an
iolist in memory.

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
  `{:collections :copied :reclaimed :live :live-bytes :threshold
  :pause-total-us :pause-max-us :pause-last-us :debug-build}` —
  for observing reclamation *and* pause behaviour: the `:pause-*` trio is
  cumulative wall time spent in this process's collections, the worst single
  pause, and the most recent one (µs). `:debug-build` is `true` when the binary
  carries debug assertions (i.e. *not* a performance build); `process-info`
  carries the per-process `:collections` count too.
- `(sched-stats)` returns the scheduler's cumulative counters —
  `{:spawned :exited :preempts :steals :migrations :workers :peak-threads}` —
  `:spawned − :exited` is the live-process figure, `:preempts` counts
  reduction-budget quantum exhaustions, `:steals`/`:migrations` count
  work-stealing activity.
- `(profile-start [hz])` / `(profile-stop)` — the **sampling CPU profiler**:
  arm at `hz` samples/sec (default 99), run the workload, and `profile-stop`
  returns a histogram — a list of `{:stack (fn-names… innermost-first)
  :count n}` maps, most-sampled first. Sampling walks each process's reified
  call stack at its next VM frame boundary after every tick: no signals, and
  near-zero cost when off. (A JIT-resident loop is attributed when it yields
  at its reduction-budget preempt; the legacy tree-walker isn't sampled.)
- `(system-monitor [pid opts])` — the **runtime event stream** (Erlang
  `system_monitor/2` shape): the kernel pushes selected runtime events to one
  subscriber process as ordinary `[:system kind subject-pid detail]` mailbox
  messages — `:gc` (a collection finished; detail
  `{:pause-us :collections :live}`, filtered by `:gc-min-pause-us`, BEAM's
  `long_gc`), `:spawn` (detail = parent pid), `:exit` (detail = the structured
  exit reason monitors see), and `:deopt` (detail = the JIT arm's fn name).
  No args reads the config; `nil` clears; `(system-monitor pid)` arms every
  event, `(system-monitor pid {:gc true :gc-min-pause-us 1000})` selects
  exactly the truthy keys. One subscriber at a time (last wins); events about
  the subscriber itself are never sent, and its death disarms the stream. Off,
  the cost is one relaxed flag load per event site. **Policy lives in
  telemetry**: `(telemetry/watch-runtime [opts])` spawns a watcher that
  re-emits each kernel event as a `[:runtime kind]` telemetry event, so
  operators consume runtime and app events through one attach/handler seam.
- `(gc-collect)` forces a collection now and returns the `gc-stats` map
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
refers all of `mod`'s public names bare; `(:use mod :only [a b])` refers just
those. A bare reference resolves **current namespace → imports → root**, so an
own-namespace definition shadows an import. `:use` auto-loads the module (it never
*fetches* a package — declared deps only). A bare top-level `(require 'mod)` only
*loads* `mod` — its names stay qualified (`mod/foo`); use a `(:use mod)` clause to
refer them bare. The header understands exactly three clauses — `(:use …)`,
`(:use-internals …)`, and `(:alias …)`; **anything else is a hard error**. (It used
to be silently ignored, so a misspelled `(:use-internal m)` or a Clojure-style
`(:require m)` looked like it imported names or granted access and did nothing at
all — the worst failure mode for a header that governs imports *and* privacy. That
silence also hid four std modules whose `(:doc "…")` header dropped their module
docstring on the floor.)

```clojure
(defmodule editor "the editor core"
  (:use editor/buffer)                 ; refer buffer's public names bare
  (:use text :only [insert]))  ; refer just text/insert as `insert`
(defn open (path) (insert (new-buffer) 0 (slurp path)))   ; insert → text/insert
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
  setters for the ones tooling extends — `(set-load-path! dirs)` /
  `(add-load-path! dir…)` for `*load-path*`, `(record-module-doc! key doc)` for
  `*module-docs*`. Writing `(def *load-path* …)` inside a module would define
  `mod/*load-path*` and the loader would never see it.

**Privacy is enforced** (ADR-146): a `foo--internal` name (any bare segment
containing `--`) is module-private. From inside *another* module, a
hand-written qualified reference to it — plain or via an `(:alias …)` — is a
**compile error at load**, and `(:use mod :only […])` refuses to import one.
Three deliberate doors stay open:

- **`(:use-internals mod)`** in a module header is the explicit grant (the
  `@testable import` seam) — tests and tightly-coupled tooling declare their
  privileged access loudly; it also refers `mod`'s public names like `(:use)`.
- **Top-level / REPL code** (no `defmodule`) is unrestricted — the
  live-hacking hatch: hot-reloading or advising a private from the REPL keeps
  working (`def` of a qualified private still rebinds it).
- **A module's own macros** may expand to its privates anywhere: enforcement
  reads the *hand-written* source, pre-expansion (macro templates live behind
  `quasiquote`, which the privacy walk skips) — the pattern the test
  framework's `describe`/`test` macros rely on.

Reflection (`eval`, `global-names`, `bound?`) still sees the flat table —
privacy governs what a module's source may reference, not what the live image
contains. The advisory checker additionally warns on private names that are
defined but never called within the file — see
[Advisory lints](#advisory-lints-non-type-warnings).
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
`doc`  `arglist`  `global-names`  `bound?`  `apropos`  `doc-search`

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
(global-names)           ;=> (… sorted list of every global …)
(apropos "rand")         ;=> (rand-float rand-int rand-seed …)  ; names containing "rand"
(apropos :shuffle)       ;=> (shuffle shuffle--acc)             ; string/symbol/keyword pattern
(doc-search "random")    ;=> ([rand-int "…"] [sample "…"] …)    ; matches docstrings, not names
```

These three are Brood over `global-names`/`doc` (`std/prelude.blsp`), and are
also exposed as `nest mcp` tools (`apropos`, `all-globals`, `doc-search`) so an
agent can explore the live image — see `docs/mcp.md`.

## Prelude

> **Reserved.** Every function and macro here is a *reserved name* — `(def map …)`
> is an error (ADR-166). Shadow one locally with `let`, or define your own inside a
> `(defmodule …)`; your own globals and your packages stay fully redefinable.

`std/prelude.blsp` is loaded at startup and is where most of the language
actually lives — the `defn` macro, the arithmetic operators, comparisons,
equality, the sequence library, and the `->`/`->>` threading macros, all defined
in Brood on top of the Rust primitive kernel. It also adds `inc` `dec`
`identity` `second` `third` `zero?` `positive?` `negative?` `abs` `max` `min`
`even?` `odd?` `sum` `product`. It's ordinary Brood — every function in it is defined with `defn`, exactly as you'd
define your own — but the *names* are reserved: it can be read, studied and copied,
not rebound (ADR-166).

## Standard library (opt-in modules)

These modules are baked into the binary but **not** loaded at startup — use
`(require 'name)` or `(:use name)` in a `defmodule` header to load one.
Run `nest doc <module>` for the full API of any module.

| Module | `require` name | What it provides |
|--------|---------------|-----------------|
| `std/file.blsp` | `'file` | Filesystem policy over the kernel's fs primitives: `read-lines`, `write-lines`, `file?`, `list-files`, `list-dirs`, `walk-files`, `path-extension`, `path-stem`. All Brood (ADR-006), no new Rust |
| `std/io.blsp` | `'io` | Output **ports** — the `Port` ability (`io-write`), `stdout-port`, `stderr-port`, `process-port`, `file-port`, `fn-port`, and the `with-out`/`with-err` redirections — so output has a first-class destination instead of only `println` (see also `std/log.blsp`) |
| `std/text.blsp` | `'text` | Plain-text transforms with no editor/buffer/IO dependency: `fill`, greedy word-wrap to a column width. Pure Brood over the string primitives, so it is reusable anywhere (fill-paragraph, wrapping help text or REPL output) |
| `std/ansi.blsp` | `'ansi` | ANSI/VT100 escape-sequence **stripping** for pipe output — `strip-ansi` removes CSI colour/cursor sequences (reading a subprocess that emits colour). For *emitting* escapes in a display frontend, see `std/editor/ansi.blsp` instead |
| `std/datetime.blsp` | `'datetime` | Gregorian calendar arithmetic: `date-new`, `date->unix`, `unix->date`, `date-add`, `date-diff`, `date-format`, `date-parse`, parse/format patterns |
| `std/encoding.blsp` | `'encoding` | Hex and Base64 encode/decode over strings (`hex-encode`, `hex-decode`, `base64-encode`, `base64-decode`) and byte vectors (`hex-encode-bytes`, `hex-decode-bytes`, `base64-encode-bytes`, `base64-decode-bytes`, plus URL-safe forms — byte-faithful, no UTF-8 round-trip) |
| `std/stats.blsp` | `'stats` | Descriptive statistics: `mean`, `median`, `variance`, `stddev`, `percentile`, `mode`, `frequencies`, `stats-min`, `stats-max` |
| `std/stream.blsp` | `'stream` | Process-based pull streams (lazy, I/O-friendly): sources (`stream-from-list`, `stream-range`, `stream-from-socket`), transformers (`stream-map`, `stream-filter`, `stream-chunk`, `stream-lines`), terminals (`stream-fold`, `stream-to-vector`, `stream-pipe`) |
| `std/url.blsp` | `'url` | URL encoding/parsing: `percent-encode`, `percent-decode`, `query-encode`, `query-decode`, `parse-url`, `build-url` |
| `std/csv.blsp` | `'csv` | CSV parse and emit: `csv-parse`, `csv-parse-maps`, `csv-emit`, `csv-emit-maps` |
| `std/uuid.blsp` | `'uuid` | UUID generation: `uuid-v4` (random), `uuid-v7` (time-ordered, RFC 9562), `uuid-nil`, `uuid?` |
| `std/template.blsp` | `'template` | `{{var}}` string templating: `render`, `render-all` |
| `std/wasm.blsp` | `'wasm` | WASM component interop (ADR-071/145): `wasm-load`/`wasm-instantiate` a sandboxed component, `wasm-call` its exports (marshalled by WIT types, fuel-metered), `wasm-call-blocking` (the offload pool), `use-native` (bind every export as a Brood fn), `wasm-exports`, `wasm-close` |
| `std/queue.blsp` | `'queue` | Purely functional FIFO queue and min-priority queue |
| `std/multimap.blsp` | `'multimap` | Multi-valued map (one key → multiple values) |
| `std/hash.blsp` | `'hash` | `sha256`/`sha1`/`sha384`/`sha512`/`md5` (hex over strings or byte vectors), raw-byte digests (`sha256-raw` … → byte vectors, for chaining over bytes), `bytes->hex` (byte seq → lowercase hex), `hmac-sha256` (RFC 2104) and raw-byte `hmac-sha256-raw`/`-sha1-raw`/`-sha512-raw` (byte-vector key+msg → byte vector, for binary-protocol auth), `hash-string` (djb2). All Brood over two Rust prims (`%digest`/`%hmac`). |
| `std/diff.blsp` | `'diff` | LCS-based sequence diff: `diff-seq`, `diff-lines`, `diff-summary`, `diff-patch`, `diff-unified` |
| `std/path.blsp` | `'path` | Path string manipulation: `join`, `split`, `basename`, `dirname`, `extension`, `stem`, `normalize`, `relative-to`, `absolute?`, `with-extension` |
| `std/system.blsp` | `'system` | OS interaction: `env`, `env-all`, `argv`, `os-type`, `cmd`, `cmd-ok?`, `cmd-out`, `halt` (whole-machine `cwd`/`hostname` are root builtins) |
| `std/crypto.blsp` | `'crypto` | Cryptography: ChaCha20-Poly1305 AEAD (`encrypt`/`decrypt`/`encrypt-str`/`decrypt-str`), `pbkdf2` (accepts a string or byte-vector password/salt — a binary salt is used as raw bytes), `random-bytes`, `random-key`, `random-nonce`, `secure=?` |
| `std/proc/agent.blsp` | `'proc/agent` | Process-backed state cell (Elixir-style Agent): `start`, `get`, `update`, `get-and-update`, `cast`, `stop` |
| `std/protocol.blsp` | `'protocol` | Behaviour contracts — the *module*-satisfies-a-contract seam: `defbehaviour` declares the ops a module must define (no value dispatch), claimed with `(:implements Name)` in a module header; `protocol-ops` is the introspection hook the checker and LSP read. Value dispatch is `ability` — `defprotocol`/`defimpl` were retired (ADR-168) |
| `std/telemetry.blsp` | `'telemetry` | Erlang-`:telemetry`-style instrumentation; handlers run in an isolated listener process: `start-telemetry`, `stop-telemetry`, `emit`, `attach`, `detach`, `detach-all`, `forward`, `handlers`, `telemetry-sync`, the `span` macro |

The following modules are also opt-in and live under `std/net/` and `std/tool/`:

```clojure
(require 'net/tcp)    ; tcp-listen / tcp-connect / tcp-send / tcp-close … (thin wrapper over the net primitives)
(require 'net/http)   ; http-get / http-post / http-request / serve / stream-response
(require 'net/sse)    ; Server-Sent Events helpers
(require 'test)       ; describe / test / assert= / is — the test framework
(require 'format)     ; printf-style string formatting
(require 'json)       ; json-encode / json-decode
(require 'regex)      ; re-match / re-find / re-replace (thin wrapper over the regex engine)
(require 'set)        ; set-specific algebra: set / union / intersection / difference / subset?
                      ;   (conj/disj/get/into/contains? on a set are prelude — no import needed)
(require 'fuzzy)      ; fuzzy string matching
(require 'log)        ; structured logging
(require 'task)       ; promise-style async tasks over processes
```

### Telemetry (`require 'telemetry`)

An Erlang-`:telemetry`-style instrumentation seam (ADR-106), written in Brood. Code
**emits** a named event with a measurements map and a metadata map; **handlers**
attached to that event run on each emit — but in an **isolated listener process**, so
a handler can never affect the emitting process:

```clojure
(defmodule my-app (:use telemetry) (:use log))

(start-telemetry)                                  ; spawn the listener once; supervise it

(attach :access-log [:http :request :stop]         ; id, event, handler
  (fn (event measurements metadata)
    (log-info (str (get metadata :method) " " (get metadata :path)
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
- **The trade-off vs. Erlang.** Erlang runs handlers inline in the caller (fast, but
  a bad handler degrades the caller). Brood chooses total emitter isolation, at the
  cost of one listener as a serialization point. Keep handlers cheap, or use
  `(forward id event pid)` to ship events to a process you own and do the heavy work
  there.
- **Events are plain Brood values** compared by structural `=` — a keyword
  (`:request`) or, Erlang-style, a vector of keyword segments
  (`[:http :request :stop]`).
- **Zero-cost when off.** `emit` with no listener started is a cheap no-op.
- `telemetry-sync` flushes (a FIFO round-trip) — handy in tests and before shutdown.

`attach`/`detach` update a shared global, so call them at startup, not concurrently
from many processes (configuration-time, as in Erlang). See `std/telemetry.blsp`.
