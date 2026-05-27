# Pattern matching

> Status: **implemented** (ADR-021). Erlang/Elixir-style pattern matching, built
> to fit the project's rules: the compiler is written in Brood (`std/prelude.blsp`),
> there is no new special form, and one mechanism is reused at every binding site.
> Subsumes two roadmap items — "Destructuring in `let`/`fn`" and "`case`" (see
> [`../ROADMAP.md`](../ROADMAP.md) Tier 2). The `let`/`fn` pattern surfaces are
> lowered to `match*` in the **compile pass** (ADR-022), which also makes a `match`
> in a hot loop expand once rather than per call.
>
> Two refinements landed slightly differently from the prose below, both noted
> inline: the failure **context** for `fn` clauses is `:fn` (not the function
> name — the name is attached after closure creation), and pattern destructuring
> of `&optional` slots is deferred (required slots only for now).

## The idea: binding *is* matching

Elixir has no assignment — only matching. `=` is the *match operator*:
`{:ok, v} = fetch()` evaluates the right side, then matches the pattern on the
left, binding `v`. The same patterns appear in `case`, function heads, `receive`,
and `with`. One concept, used everywhere a name gets bound.

We want that feel, not the literal mechanism — because the literal mechanism
**cannot** be copied into a Lisp, and the reason is exactly what makes Brood a
Lisp:

- Elixir distinguishes a *pattern* from a *constructor* **by position** — `{:ok, v}`
  left of `=` is a pattern (binding `v`); the same text on the right is a
  constructor (using `v`). Different brackets (`{}` `[]` `%{}`) vs calls `f()`
  carry the distinction.
- In Brood, code is data: `(:ok x)` is *just a list*, identical in syntax to a
  call and to quoted data. A bare `x` already means "look up `x`." **Nothing in
  the text marks a pattern.** So we cannot overload `=` to mean match — `=` is a
  plain prelude function (ADR-008), and `(= a b)` evaluates both operands.

The Lisp-faithful translation: don't put matching on an operator; put **one
pattern grammar at every binding form**, and let those binds be *refutable* (fail
→ raise) where that's useful. You give up "`=` is the binder"; you keep the part
that actually mattered — the same pattern language in `let`, `fn`, `receive`, and
`match`.

## One compiler, many sites

Everything below is the same pattern compiler used four ways:

```
compile(pattern, target, on-success, on-fail) -> code
```

| Surface | `on-fail` is… |
|---|---|
| `match` | fall through to the next clause; last clause raises |
| refutable `let` | raise (this is Elixir's `=`) |
| `fn` / `defn` clauses | try the next clause; no clause raises |
| `receive` clauses | leave the message queued and try the next; if none match, suspend (or run the `after` timeout) — see below |

Build the compiler once, in Brood, and each surface is a thin macro over it.

## The pattern grammar

One vocabulary, shared by every site:

| Pattern | Matches / binds |
|---|---|
| `_` | anything; binds nothing (wildcard) |
| `x` | anything; **binds** `x` |
| `42` `3.14` `"s"` `true` `false` `nil` `:kw` | a literal, by `=` |
| `'sym` | the literal symbol `sym` (bare symbols bind, so quote to match) |
| `(p1 p2 …)` | a list of exactly that length, element-wise |
| `(p1 & rest)` | head(s) + tail bound — reuses the `&` rest marker |
| `[p1 p2 …]` | a vector of exactly that length — the Erlang *tuple* |
| nested | patterns compose to any depth |

**It's additive.** Every binding form Brood has today is already the
all-bare-symbols degenerate case: `(let (a 1 b 2) …)` is two trivial patterns,
`(defn f (x y) …)` is a two-pattern parameter list. Nothing existing changes;
the binders just get more expressive.

### The one trap: a bare symbol always binds

The mistake to anticipate — for humans, and especially for generated code: a bare
symbol in a pattern **binds** (and shadows). It does *not* test against a
same-named value. So this is wrong:

```clojure
(def none :none)
(match x
  (none   0)        ; does NOT compare x to `none` — it binds `none` to x
  ([:v n] n))
```

Match a *known* value one of three ways, idiomatic first:
- **a keyword tag** — `:none`, `[:ok v]`. Keywords are self-evaluating literals, so
  they match by value; this is what makes the tagged-vector idiom safe by default.
- **a quoted symbol** — `'none` matches the symbol `none`.
- **a pin** — `~expr` matches the current value of `expr` (see Pin).

To stop a silent bind-where-you-meant-match, the compiler **rejects unreachable
clauses**: an irrefutable clause (its pattern a bare symbol or `_`, with no guard)
must be last — any clause after it is dead code and is a compile-time error.

### Tagged data: lists vs vectors

Erlang messages are tuples (`{:ok, X}`); Brood's fixed-arity datatype is the
vector, so a vector pattern `[:ok v]` is the natural tuple analog. Lists stay the
sequence analog (`(h & t)` is `[H|T]`). Both are valid patterns, but **vectors are
the idiom for tagged data** — the decisive reason is *constructor/pattern
symmetry*: a vector literal `[:ok v]` both **builds** and **matches** (the same
text, both directions), exactly the Elixir property. A list can't: unquoted,
`(:ok v)` is a call, so you'd build with `(list :ok v)` but match with `(:ok v)` —
the two diverge. Lists remain the sequence pattern; vectors are tuples/records.

## The surfaces

### `match`

`match` clauses are **wrapped**: `(pattern [:when guard] body…)` — the same clause
shape `fn` and `receive` use, so there is one "clause" concept across the language:

```clojure
(match msg
  ([:say text]      (println text))
  ([:add a b]       (+ a b))
  ((x & xs)         (str "head " x ", rest " xs))
  (n :when (int? n) (handle-int n))
  (_                (println "unknown")))      ; explicit catch-all
```

Wrapped, not flat `pattern body` like `cond`, for three reasons:
- a **guard** sits cleanly *between* pattern and body (`pattern :when g body`); flat
  would jam it into the pattern, where `(x :when g)` is indistinguishable from a
  three-element list pattern;
- a clause **body can be several forms** (implicit `do`, like every other body in
  the language);
- `match` / `fn` / `receive` then share **one** clause grammar — less for a human or
  an LLM to remember, and misuse fails loudly (a non-`(pattern …)` clause is a
  compile-time error, not a silent mis-parse).

Pure Brood, pure macro — no core change. Each chosen body lands in tail position of
the generated `cond`/`let`, so **a `match` in tail position stays TCO-safe**:
match-driven loops and receive loops won't overflow.

`case` (dispatch on a value, literal patterns) is just `match` with literal
patterns — no separate construct.

### Refutable `let`

`let` keeps its shape — flat `pattern value …` pairs, sequential — and a binding
target becomes a pattern. A bare symbol is the degenerate pattern, so today's
code is unchanged:

```clojure
(let (a 1                       ; trivial pattern (unchanged)
      [:ok v] (fetch key)       ; refutable: raises if fetch isn't [:ok _]
      (x & xs) (range 10))      ; destructure
  (use a v x xs))
```

This is Brood's `=`: a refutable bind that raises on mismatch. When you want to
*handle* a non-match instead of raising, reach for `match` (a future `with` could
add Elixir-style chained refutable binds with an `else`).

### `fn` / `defn` clauses (no `defmatch`)

`defn` is already `(def name (fn …))` — a prelude macro. So multi-clause lives on
**`fn`**, and `defn` inherits it unchanged; there is no separate `defmatch`.

**Parameter lists stay lists** (ADR-010) and the parameter grammar is unchanged —
`&optional` (with defaults) and `& rest` remain list-level markers, named `&key`
is still deferred (ADR-011). The one new thing: each required or optional **slot
may be a pattern** instead of a bare symbol, so vectors (tuples) and lists
(sequences) can be destructured per argument:

```clojure
(defn f (a [x y] &optional (c 10) (p [0 0])) …)
;  a      arg 0, binds
;  [x y]  arg 1, destructured as a 2-tuple
;  c      optional, default 10
;  p      optional, defaults to [0 0]  (a bare symbol — optional *slots*
;         can't yet be patterns; see the note above)
```

Multi-clause dispatches by pattern (and guard), in order — the canonical Erlang
shape:

```clojure
(defn fac
  ((0)  1)
  ((n)  (* n (fac (- n 1)))))
```

Single- vs multi-clause is one rule:

> **`fn` is multi-clause iff *every* form after the name is a clause** — a list
> whose head is itself a parameter list (a list or vector). Otherwise it's
> single-clause.

This resolves the cases by intent:

| Form | Read as |
|---|---|
| `(fn (x y) (+ x y))` | single — arg0 head `x` is a symbol, not a param-list |
| `(fn ((a b) c) (+ a b))` | single — body `(+ a b)` isn't clause-shaped, so not *all* forms are clauses; first param `(a b)` destructures |
| `(fn ((0) 1) ((n) (* n …))) ` | multi — every form is a clause |

The only thing it can't express is a single-clause fn with a compound first
parameter *and* an empty body — useless, so the cost is nil. Clauses are
same-arity **pattern + guard** dispatch (Erlang), *not* Clojure-style multi-arity
dispatch, which ADR-011 deferred — we are not reopening that.

**Footgun:** the form right after the name is always the parameter *list*, so a
vector there is the whole list, not a tuple param — `(defn g [x y] …)` is two
params (ADR-010's vector-param leniency), `(defn g ([x y]) …)` is one tuple param.
Since vectors now read as tuples, "parameter lists are lists" is the firm idiom
(we may retire the vector-param-list leniency).

**Style:** dispatching on a *single* tagged argument reads better as `match` in the
body than as clause heads (which need the wrapping paren and get dense). Multi-clause
heads earn their keep dispatching on *several* parameters at once.

```clojure
(defn area (shape)                  ; preferred for one tagged argument
  (match shape
    ([:circle r] (* 3.14 r r))      ; clauses are wrapped (decision 6), not flat
    ([:rect w h] (* w h))))
```

## Power features

These are layers on the same compiler.

- **Guards** — `:when` after a pattern; the clause matches only if the guard is
  truthy. `:when` (not bare `when`, which is a special form) avoids any clash.

  ```clojure
  (match n
    (n :when (> n 0) :positive)
    (0               :zero)
    (_               :negative))
  ```

- **Non-linear patterns** — a repeated variable is an equality constraint:
  `[x x]` matches only a 2-vector of equal elements. (Erlang does this
  implicitly; the compiler tracks already-bound names and emits an `=` check on
  re-occurrence.)

- **Pin** — match against the *current value* of a name already bound outside the
  pattern (Elixir's `^x`). Brood has no `^` reader syntax, but `~` already reads
  as `(unquote …)`, and "drop to evaluation" is the same intuition it has in
  quasiquote. So inside a pattern, `~x` (or `~(expr)`) means "match the value of
  `x`":

  ```clojure
  (let (expected :ok)
    (match resp
      ([~expected v] v)        ; matches only when the tag equals `expected`
      (_             :other)))
  ```

  Non-linear vs pin are complementary: non-linear constrains two positions
  *within* one pattern to be equal; pin constrains a position to a value from
  *outside* the pattern. **Decided:** `~x` (no reader change; the "drop to
  evaluation" intuition is the same one `~` has in quasiquote).

## Errors

Clear errors are a requirement, and there are two kinds: **compile-time** (raised
while the `match` macro expands) and **runtime** (a match that finds no clause).
Neither has source locations yet — the reader tracks positions but doesn't attach
them to values (a separate roadmap item) — so messages are made self-describing.

**Runtime — nothing matched.** A `match`, a refutable `let`, or a `fn` call that
matches nothing **crashes** (raises), Erlang-style: a process handed a message it
can't match should die loudly, not limp on with `nil`. Add an explicit `_` clause
to make a match total. The error is **structured and catchable**, and carries the
value that didn't match, the patterns that were tried (the macro has them as data
at expand time, so it quotes them in), and a context label — enough to read on its
own. Target messages:

```
match: no clause matched 42 — tried [:ok _], [:err _]
let: pattern [:ok v] did not match [:err 404]
area: no clause matched arguments (7 :hexagon) — clauses ([:circle r]), ([:rect w h])
```

Structured means a handler can match on the failure itself:

```clojure
(try (decode x)
  (catch e
    (match e
      ([:match-error :let pat got] (recover pat got))
      (_                           (throw e)))))
```

**Compile-time — malformed or dead patterns** fire before the code ever runs:
- a malformed pattern — `(a & b c)` → `match: '&' must be followed by exactly one
  tail pattern`;
- an **unreachable clause** after an irrefutable one → `match: unreachable clause
  after catch-all`;
- a clause that isn't `(pattern body…)`, or a `:when` with no guard.

*Infra note.* `throw` carries a payload value, but an uncaught error's printed
message is just that value's display form — there is no error today holding *both*
a custom message and a structured payload. So the thrown value is a tag like
`[:match-error <context> <value> <patterns>]`: it is matchable, and its display
reads legibly. A small future "error with message + payload" (near the maps work)
would let the printed line be a bare sentence; not needed for v1.

## Implementation sketch

- **The matcher is Brood.** A pattern→code compiler in `std/` (a macro plus
  expand-time helper `defn`s, exactly like the threading macros compute at
  expansion time) emits nested `if`/`let` over existing primitives: `pair?`,
  `first`, `rest`, `nil?`, `vector?`, `vector-ref`, `vector-length`, and `%eq`.
  Fresh temporaries via `gensym`. No Rust, no new builtin. This is the
  `try`/`catch` precedent (a macro over a primitive) applied again.
  - *Hygiene:* the generated code references those primitives by bare name, which
    a local binding could shadow (Lisp-1, unhygienic macros — ADR-009). Equality
    uses the kernel `%eq` rather than `=` by convention, since `%`-names aren't
    rebound; the rest (`first`/`rest`/…) remain shadowable until macro hygiene
    lands. In practice, don't bind `first`/`rest`/`pair?`/… as locals around a
    `match`.

- **`match` needs no core change** — it's a plain macro.

- **Good errors come from the macro itself.** Because the matcher is Brood code
  that holds the literal patterns as data, it can both emit the compile-time checks
  above while expanding *and* embed the value, the tried patterns, and a context
  label into the runtime failure — no special runtime support needed.

- **`let` and `fn` are lowered in the compile pass, not the evaluator.** They
  are special forms in `eval.rs`, matched *before* macroexpansion, so they can't
  be macros. The design's Option A delegated at runtime (rewrite to `match` and
  `continue 'tail`); the **implementation does the same rewrite one phase
  earlier**, in the `macroexpand_all` compile pass (ADR-022): a `let` with a
  non-symbol target, or a multi-clause / pattern-parameter `fn`, is desugared to
  `match*` once at definition. So in the common case eval's `let`/`fn` see only
  plain symbol binds, the matching *logic* stays in the Brood `match*` engine, and
  a pattern binder in a hot loop expands once rather than per call. Tail position
  is preserved (the body lands in `match*`'s tail), so `tail_calls_do_not_overflow`
  holds — including for multi-clause recursion like `fac`.

  *Concretely:* `(let (pat v) body…)` → `(match* :let v (pat body…))`;
  `(fn (clause…)…)` → `(fn (& g) (match* :fn g clause…))`;
  `(fn (a [x y]) body)` → `(fn (a g) (match* :fn g ([x y] body)))`. `defn` is now
  a pure forwarder to `fn`, so it inherits both forms.

  **Eval keeps the Option-A fallback too.** A pattern binder can still reach the
  evaluator *unlowered* — built inside a quasiquote unquote (the compile pass
  leaves quasiquote opaque) or produced by a macro expanded lazily within its own
  defining form. So `eval`'s `let`/`fn` also detect a non-symbol target / a
  clause-shaped `fn` and lower it on the fly (rewrite through `macroexpand_all`,
  then `continue 'tail`). The compile pass is the fast common path; this fallback
  is the correctness backstop (otherwise such a binder failed with a misleading
  "expected a symbol"). The common all-symbol case is detected away cheaply, so an
  ordinary `let`/`fn` never pays for it.

- **Code-size note.** The textbook risk is duplicating the fail-continuation into
  every sub-pattern failure point (exponential blowup on deep nesting). Patterns
  are shallow in practice; if it bites, bind the fail-continuation as a thunk.
  Decide when measured, not now.

## `receive` (implemented — selective)

`receive` is the fourth surface over the pattern compiler. It is a macro
(`std/prelude.blsp`) that reuses **`match-build-from`** with the no-match
continuation set to `nil` (instead of the structured throw), wrapping each clause
body in a thunk. The result is a *matcher* function — given a message it returns
the body-as-a-thunk on a match, or `nil` otherwise — which the `%receive`
primitive runs over the mailbox: scan in order, **remove + run the first match,
leave non-matching messages queued**. That is true Erlang **selective receive**
(no head-of-line blocking), not the simpler "match the next message or fall
through" form first sketched here.

```clojure
(receive
  ([:say text]      (println text))
  (n :when (int? n) (handle-int n))
  (after 5000       (throw [:timeout])))   ; optional timeout; catchable via try/catch
```

The `on-fail` for a `receive` clause (the table above) is thus *leave queued and
try the next message; if none match, suspend (or, with `after`, run the timeout
body)*. `(receive)` with no clauses takes the next message. See
[`concurrency.md`](concurrency.md) / [`scheduler.md`](scheduler.md) for the
runtime side (the green-process suspend + the receive timer) and ADR-027.

## Decisions (review pass)

1. **Tagged data → vectors** `[:ok v]`. The decisive win is constructor/pattern
   symmetry — the same literal builds *and* matches (a list `(:ok v)` can't:
   unquoted it's a call). Lists remain the sequence pattern. Both still work as
   patterns; vectors are the documented idiom.
2. **Pin → `~x`** (no reader change; same "drop to evaluation" intuition as
   quasiquote).
3. **Failure → crash, with a structured value.** A non-matching `match`,
   refutable `let`, or `fn` call raises (catchable via `try`/`catch`); add a `_`
   clause to make it total. The value is structured (e.g. `[:no-match v]`) so a
   handler can match on it. Erlang "let it crash"; fits the supervision model.
4. **Phasing → not a design question** (build order). Likely one PR; the shared
   compiler + `match` is the foundation, so it lands first regardless.
5. **`let`/`fn` factoring → Option A (delegate).** Touch the special forms, but
   only to *delegate*: a non-symbol binding target rewrites to `(match …)` and
   re-enters the eval loop. One matcher, in Brood — the part you'll want to extend
   later (map patterns, custom extractors) must stay redefinable, not frozen in
   Rust.
6. **Clause shape → wrapped** `(pattern [:when guard] body…)` for `match` /
   `receive` / `fn`. One clause concept; guards and multi-form bodies fit cleanly;
   misuse is a loud compile-time error, not a silent mis-parse. (`let` stays flat
   `pattern value …` — it interleaves bindings, it isn't a clause construct.)
7. **Errors → structured, catchable, self-describing.** No-match crashes with a
   tagged value carrying value + tried patterns + context; the macro also raises
   compile-time errors for malformed patterns and unreachable clauses. A bare
   symbol binds (the one trap) — match a known value with a keyword, `'sym`, or
   `~pin`; an unreachable clause after an irrefutable one is rejected.

### Parameter grammar (resolved)

A parameter list stays a **list** (ADR-010); `&optional`/`& rest` are unchanged;
named `&key` stays deferred (ADR-011). Each required/optional **slot** may be a
pattern. A top-level vector after the name is still the whole param list, so wrap
a single tuple param: `(defn g ([x y]) …)`. Clauses are same-arity pattern+guard
dispatch, not arity overloading.

## Relationship to the roadmap

This single feature subsumes Tier-2 "Destructuring in `let`/`fn`" and "`case`",
and sets up `receive` clauses. It records as an ADR in
[`decisions.md`](decisions.md) once the design here is accepted.
