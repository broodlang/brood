# The Design Principles of the Elixir Type System — reference notes

**Authors:** Giuseppe Castagna (IRIF, Université Paris Cité & CNRS), Guillaume Duboc
(IRIF / Remote Technology), José Valim (Dashbit).
**Venue:** *The Art, Science, and Engineering of Programming*, 2024, vol. 8, issue 2,
article 4 (`programming-journal.org/2024/8/9/`). arXiv:2306.06391.

**Source of this document.** Fetched from the arXiv PDF
(`https://arxiv.org/pdf/2306.06391`) on 2026-05-31 and text-extracted with
`pdftotext -layout`. This file is a *detailed technical summary* assembled by reading
the full extracted text, with definitions / rules / formulas quoted **verbatim** where
they matter (quotes use the paper's notation; ASCII transliterations of the math
symbols are noted inline). The full raw extraction is kept alongside this file as
`_elixir-types-raw.txt` (39 pp.) for verbatim cross-checking. Companion papers the
authors cite for the omitted proofs/algorithms: **[8]** (records/maps), **[9]**
(function arity, guard analysis, gradual typing — *this is where the strong-arrow and
`?`-propagation rules are proved*), **[30]** = Lanvin's PhD thesis (set-theoretic
gradual typing), **[11]** (materialization), **[12]/[13]** (occurrence typing with
set-theoretic types).

The overall thesis: Elixir's planned type system is a **transposition of CDuce's
polymorphic semantic-subtyping type system** to the Erlang family, *extended* to handle
four things CDuce/prior semantic subtyping did not: **(a) function arity**, **(b)
guards**, **(c) a unified records+dictionaries map type**, and **(d) a new sound
gradual-typing discipline** that does *not* insert runtime casts but instead leans on
the type-checks the BEAM (and the programmer's guards) already perform — the
"strong arrow" idea.

---

## 1. Set-theoretic types

### Types are sets of values; subtyping is set inclusion

> "Unions, intersections, and—see later on—negations are called set-theoretic types,
> insofar as they can be thought of in terms of sets: if we think of a type as the set
> of all values of that type … then the union of two types is the set that contains the
> union of their values …, the intersection of two types is the set that contains the
> values that are in both types …, and, finally, the negation of a type is its
> complement, that is, it contains all the (well-typed) values that are not in the type
> (e.g., a value in `not integer()` is any value that is not an integer)." (§2.2)

Subtyping **is** semantic set inclusion: `s <: t` iff every value of `s` is a value of
`t`. Two types each a subtype of the other are *equivalent* (denote the same set).
Crucially this is a **semantic** (model-theoretic), not syntactic, subtyping — which is
what lets distributivity/commutativity laws hold:

> "The advantage of interpreting types as the set of their values is that types satisfy
> the distributivity and commutativity laws of their set-theoretic counterparts." (§2.2)

Worked example of why semantic > syntactic: the product-union factorization
`{s1,t} or {s2,t}` ≡ `{s1 or s2, t}`. Their checker accepts a program treating
`{integer() or string(), boolean()}` and
`{integer(),boolean()} or {string(),boolean()}` as equivalent, whereas:

> "languages that use a syntactic definition of subtyping, such as Typed Racket, Flow,
> or TypeScript, accept the application `f(x)` but reject the typing of `apply`: they
> cannot deduce that `t()` is a subtype of `s()`." (§2.2)

### Top, bottom, connectives — the formal grammar (Figure 1, §4.1)

The core calculus's type grammar (transliterating the symbols: `∨` = union, `¬` =
negation, `→` = arrow, `{ t }` = tuple, `1fun`/`1tup` = the top function / top tuple
types, `α` = type variable, `c` = a constant used as a **singleton** type):

```
Base types   b ::= int | atom | 1fun | 1tup
Types     t,s ::= b | c | α | t → t | { t } | t ∨ t | ¬t
```

Intersection and the lattice bounds are **encoded**, not primitive:

> "two connectives union and negation (∨, ¬), with intersection ∧ encoded as
> `t1 ∧ t2 = ¬(¬t1 ∨ ¬t2)`. We also encode the top type 1, the type of all values, as
> `1 = int ∨ atom ∨ 1fun ∨ 1tup`, and the bottom type O as `O = ¬1`: they correspond to
> Elixir's `term()` and `none()` types, respectively." (§4.1)

So at surface-syntax level: `term()` = top, `none()` = bottom = `not term()`, and
conversely `term()` ≡ `not none()` (§2.2). `and`/`or`/`not` are the surface spellings of
`∧`/`∨`/`¬` (chosen because they already exist in Elixir guards; §5 "Type Syntax").

### Base / atomic types

In the *full* surface language: `integer()`, `float()`, `atom()`, `binary()` (the type
of strings — "binary() is the type for strings", §2.3), `boolean()`, plus the BEAM
runtime types usable as **dictionary key domains**: `integer(), float(), atom(),
tuple(), map(), list(), function(), pid(), port(), reference()` (§3.3). **Singleton
types**: every atom (`:ok`, `true`, `false`, `nil`, …) is a type containing exactly that
one constant — "the atoms `true`, `false`, and `nil` … are also types, called singleton
types, because they contain only the constant/atom of the same name" (§2.2). In the
*minimized core calculus* the base types collapse to just `int | atom | 1fun | 1tup`.

### Structured types ARE first-class

Yes — arrows, tuples, lists, and maps are all first-class type constructors with proper
set-theoretic meaning, freely combinable with `∨/∧/¬`:

- **Function arrows** `t → t`. Set meaning: "a value in the intersection
  `(integer()->integer()) and (boolean()->integer())` is a function that both maps
  integers to integers and maps booleans to integers" — i.e. the arrow type denotes the
  set of functions respecting *all* its arrow components. (See §5 below on multi-clause /
  intersection-of-arrows and arity.)
- **Tuples / products** `{ t1, …, tn }` (Elixir's curly-brace tuples). Products
  distribute over unions as sets (the `{s1 or s2, t}` example).
- **Lists** with element types: `[a]` is the type of lists of `a`. `[]` is the singleton
  empty list; `[a] and not []` is non-empty lists. Recursive list/tree types are
  expressible: `type tree(a) = (a and not list()) or [tree(a)]`. The decision that
  `[tree(a)] <: tree(a)` falls straight out of the union definition.
- **Maps/records** — unified, §3.3, summarized in point 6 below.

The paper is explicit that this is the *whole point* relative to TypeScript/Flow: the
structured connectives obey real algebra, so e.g. an intersection of arrows is a genuine
overload type, not a syntactic annotation.

---

## 2. The `dynamic()` type and gradual typing (the crux)

### What `dynamic()` is

> "we introduce the type `dynamic()`, which essentially puts the type-checker in dynamic
> typing mode. … the programmer can think of `dynamic()` as a type that can become at
> run-time (technically, that *materializes into*: see [11]) any other type: an
> expression of type `dynamic()` can be used wherever any other type is expected, and an
> expression of any type can be used where a `dynamic()` type is expected since, in both
> cases, `dynamic()` may become at run-time that type." (§3.4)

In the core calculus `dynamic()` is the new base type written **`?`** (§4.3). It is a
*new basic type that can occur inside other type expressions* (e.g.
`dynamic() -> dynamic()`, `{dynamic(), integer()}`, `integer() and dynamic()`), not
just a standalone annotation (§3.4).

### `dynamic()` vs `term()` vs `none()` — the key distinction

`none()` = empty set. `term()` = set of all values = top. `dynamic()` is **neither** —
it is the gradual "unknown". The often-made confusion is `dynamic()` ≈ `term()`, and the
paper kills it precisely (footnote 12, §3.4):

> "Oversimplifying, one can consider `dynamic()` to be both a supertype and subtype of
> every other type (while `term()`, which is often confused with `dynamic()`, is only the
> former) **with a caveat: subsumption does not apply to `dynamic()`** since we cannot
> consider an expression of a type different from `dynamic()` to be of type `dynamic()`:
> the application of `dynamic()->dynamic()` to an integer is well-typed **because the
> arrow type materializes into `integer()->dynamic()` and not because `integer()`
> materializes into `dynamic()`**."

This is the single most important conceptual point for an implementer: **`dynamic()` is
both a sub- and a super-type of everything, but it is NOT a top type and subsumption is
NOT sound for it.** You do not "widen" a known `integer()` to `dynamic()`. Instead the
*surrounding* type (here the arrow) materializes into a more precise non-gradual type
that makes the use well-typed. Treating `dynamic()` as plain `term()`-with-subsumption
is exactly the unsoundness trap.

### Consistent subtyping is DERIVED, via materialization + ordinary subtyping

This is the headline design choice: **there is no separate Siek–Taha consistency
relation.** They follow Lanvin [30] / [11]: define a **precision** (materialization)
relation `≼` over types and reuse the *ordinary* subtyping `≤` on **non-gradual** types
(types with no `?`). "Consistent subtyping" is then the composition
materialize-then-subtype, read off the typing rules rather than axiomatized separately.

> "This approach was developed for set-theoretic types in Lanvin's PhD thesis [30] whose
> results we use here to define subtyping and precision relations using the subtyping
> relation on non-gradual types (i.e., types in which `dynamic()` does not occur)." (§3.4)

**Precision / materialization** `≼` (§4.3):

> "a type `t` is more precise than a type `s`, written `s ≼ t`, if `t` can be obtained by
> replacing in `s` some occurrences of `?` by other types."

(Footnote 14: they actually use Lanvin's *semantic* `≼` that respects type equivalence,
so e.g. `({?, int} ∨ {1fun, ?}) ≼ {1fun, int∨atom}` and `¬? ≼ ?`.)

The application rules glue precision and subtyping together. For an argument of type `s'`
applied to a function expecting `s`, the side condition is the chain
**`s' ≼ s1 ≤ s2 ≽ s`** — i.e. *materialize* the actual and the formal to some non-gradual
`s1`, `s2` related by ordinary subtyping. That chain *is* consistent subtyping,
constructed from `≼` and `≤`. (Rules quoted in §3/§5 below.)

> "`dynamic()` is a new basic type … The addition of '`?`' is then handled by defining a
> precision relation `≼`. … Whenever we need to use the precision relation to type an
> application, we propagate `?`." (§4.3)

### `dynamic()` (no bound) vs `dynamic(T)`

Unbounded `dynamic()` = the materialization range is *all* types (it can become
anything). A **bounded** `dynamic(T)` is expressed in this system as the **intersection
`T and dynamic()`** (= `t ∧ ?`). The paper develops this intersection-with-dynamic form
at length because it is what makes gradual code maximally typable:

> "An expression of type `t() and dynamic()` can be used not only in all contexts where
> an expression of type `t()` is expected, but also in all contexts where a **strict
> subtype** of `t()` is expected (in which case the use of `dynamic()` will be further
> propagated)." (§3.4)

So `integer() and dynamic()` is "an integer as far as the static checker is concerned,
but still carrying a `?` so it may be materialized down to a strict subtype and passed
where a narrower type is demanded." That is the bounded-dynamic semantics, expressed as
an ordinary intersection rather than a special form.

### How `dynamic()` composes with the connectives

It is a base type, so it nests freely and composes through the *same* set-theoretic
algebra — the system propagates `?` structurally:

- **Intersection** `t ∧ ?` — bounded dynamic (above); the workhorse for typability.
- **Union** `? ∨ t` — a value either of known `t` or unknown.
- **Negation** `¬?` — appears; footnote 14 gives the equivalence `¬? ≼ ?`. (Negation of
  dynamic is a known soundness minefield — see point 7.)
- **Inside structured types** — `{dynamic(), integer()}`, `dynamic() -> dynamic()`,
  `(dynamic() -> dynamic()) -> _`, etc.

The propagation result for an unannotated function with no guard (so its parameter is
assumed `dynamic()`): for `def foo3(x), do: {id_weak(x), id_strong(x)}` the system
deduces

```
dynamic() -> {dynamic(), (integer() and dynamic())}
```

— the *weak* call leaves `dynamic()`, the *strong* call yields `integer() and
dynamic()`. (Contrast a classic cast-inserting gradual system, which would deduce
`{integer(), integer()}` *but* would have rewritten the compiled code to insert two
runtime integer checks — which this system forbids; §3.4.)

### Gradual guarantee / soundness (sound gradual typing without casts)

> "Even in the presence of `dynamic()` type annotations, our type system guarantees that
> if an expression is given type, say, `integer()`, then it will either diverge, or
> return an integer value, or fail on a run-time type-check verification. This safety
> guarantee characterizes the approach known as **sound gradual typing**." (§3.4)

The departure from textbook sound gradual typing: they require that **adding types must
not change Elixir's compilation** (no inserted casts; §5). So soundness is recovered by
*accounting for the checks that already happen* — the BEAM's own runtime type tests plus
the programmer's guards. The strong-arrow analysis (point 3) is what makes this rigorous.

---

## 3. Strong vs weak arrows (the novel gradual-typing technique)

The distinction is **whether a function performs the runtime type check on its
argument** — which decides whether `dynamic()` may be *stopped* (refined to a precise
codomain) or must be *propagated*.

Two definitions, *same static type* `integer() -> integer()`, differ at runtime:

```elixir
$ integer() -> integer()        $ integer() -> integer()
def id_weak(x), do: x           def id_strong(x) when is_integer(x), do: x
```

> "from a runtime perspective, the two definitions above differ as the latter checks
> that its argument is of type `integer()`, while the former does not." (§3.4)

**Strong arrow** `(s → t)*`:

> "We refer to the function `id_strong` as having a 'strong' function type, since it
> guarantees that when applied to an argument that is not within its domain, it will
> either (i) return a result within its codomain, or (ii) fail on a dynamic type
> check—performed by the Erlang VM or inserted by the programmer—, or (iii) diverge."
> (§3.4)

**Weak arrow**: no such guarantee — `id_weak` applied off-domain *does* return an
off-codomain value. Built-in operations (field selection, tuple projection, `+`, …) are
strong by construction because the BEAM checks their operands.

**The strongness rule `(λ*)` and the strong-typing judgment `e ⦂⦂ t`** ("`e` strongly
ensures it returns a result in `t`"), §4.3. Transliterating (`¬s` = negation, `⦂⦂` = the
strong judgment, `1` = top, `int` = integer):

```
   Γ, x : s ⊢ e : t     Γ, x : ¬s ⊢ e ⦂⦂ t                  Γ ⊢ e1 ⦂⦂ 1   Γ ⊢ e2 ⦂⦂ 1
(λ*) ─────────────────────────────────────        (add) ──────────────────────────────
        Γ ⊢ λ(x.e) : (s → t)*                              Γ ⊢ e1 + e2 ⦂⦂ int
```

Read `(λ*)`: a function is *strong* of type `(s → t)*` if, even under the hypothesis
that the parameter is **outside** the domain (`x : ¬s`), the body still *strongly*
guarantees the codomain `t` (`e ⦂⦂ t`) — i.e. some runtime check inside the body will
catch a bad argument before an off-codomain value can escape. `(add)` is a leaf: `+`
strongly yields `int` because the VM checks its operands. For strong functions the
`case` rule **does not check exhaustiveness** — "if no branch matches, then the case
fails and the expression is strong" (a runtime `case_clause` error is itself a check).

**Applying a `dynamic()` value / propagation rules** (§4.3). Two application rules,
strong vs weak; both use the precision-then-subtyping chain `s' ≼ s1 ≤ s2 ≽ s`:

```
 Γ ⊢ f : (s→t)*   Γ ⊢ e : s'   s'≼s1 ≤ s2≽s          Γ ⊢ f : s→t   Γ ⊢ e : s'   s'≼s1 ≤ s2≽s
 ───────────────────────────────────────────         ─────────────────────────────────────────
            Γ ⊢ f(e) : ? ∧ t                                    Γ ⊢ f(e) : ?
```

- **Weak** function applied where `?` must be propagated → the whole application is just
  `?` (`dynamic()`): you've lost all static information.
- **Strong** function → the result is the function's codomain **intersected with `?`**:
  **`? ∧ t`** (`t and dynamic()`). The intersection-with-`?` is the deliberate leniency
  knob: it keeps the precise codomain `t` *and* a `?` so the result can be materialized
  down to a strict subtype of `t` and flow into narrower contexts.

> "the type `dynamic()` is propagated by the type system through the various calls of
> functions such as `id_weak`, but it is **stopped** when it goes through a function with
> a strong arrow type, such as `id_strong`." (§3.4)

So **how applying a `dynamic()` value is handled**: the *function's* arrow type
materializes (per footnote 12, the arrow becomes a precise non-gradual arrow), the
argument's `?` matches via `≼`, and the result is `?` (weak) or `? ∧ codomain` (strong).
A *non-function* used as a function, or a value used where a function arrow is required
(`foo2({7,42})`), is statically rejected — `dynamic()` does not excuse that.

**Soundness keeper:** because `?` is *only* refined to a precise codomain when the
function is provably strong (its body's checks, or the VM's, guarantee the codomain),
the static type `integer()` you read off a strong application genuinely holds *unless the
program errors at runtime* — exactly the (i)/(ii)/(iii) guarantee. A weak function never
gets to claim a precise codomain from a `?` argument, so no false "it's an integer"
claim can leak. The "dynamic = the set of values the runtime checks" intuition is made
operational by tying refinement to provable runtime checks rather than to a syntactic
cast.

Note (current limitation): strong types are *internal only* — "All this is currently
transparent to the programmer … A possible extension … would be to allow the programmer
to specify whether higher-order parameters require a strong type or not." (§3.4)

---

## 4. Guards, occurrence typing / narrowing

### Guards ARE types

A central novelty (absent from prior semantic-subtyping work): a guard's accepted value
set is read back as a *type*, and used both to **infer** parameter types and to
**narrow**.

> "our system … can precisely express (most) guards in terms of types, in the sense that
> the set of values that satisfy a guard is the set of values that belong to a given
> type: … the set of all values that satisfy the guard `is_integer(person.age)` coincides
> with the set of values that have type `%{age: integer(), ...}`." (§3.2)

> "deducing the type of the parameters of a function by examining its guards is just yet
> another application of narrowing where the function parameters are initially given the
> type `term()` and narrowed by the types deduced for the guards." (§3.2)

So `def negate(x) when is_integer(x)` infers parameter type `integer()` with no
annotation; `is_integer(person.age)` infers `%{age: integer(), ...}` (an *open* record).

### Narrowing in branches (occurrence typing) — both branches, including the else

> "Narrowing is the typing technique that consists in taking into account the result of a
> (type-related) test to refine (i.e., to narrow) the type of variables in the different
> branches of the test." (§3.2)

For `negate_alt` with `(integer() or boolean())` parameter and
`if is_integer(x), do: -x, else: not x`, the checker narrows `x` to `integer()` in the
`do` branch **and to `boolean()` in the `else` branch** — i.e. it applies the *negative*
information `not integer()` in the else, intersected with the prior type. So yes: it
narrows in **both** the success and failure branches, and the failure branch uses the
*complement* of the tested type (this is exactly where set-theoretic negation pays off —
the else type is `prior ∧ ¬tested`).

It narrows **non-variable test expressions** too — variables occurring *inside* the
tested expression, not just a bare variable:

> "our system is also able to narrow the type of the variables that occur in the
> expression tested by a 'case' or a 'if', even if this expression is not a single
> variable (some exceptions apply though: see future works)." (§3.2)

e.g. testing `r.output == :ok` / `is_atom(r.message)` narrows `r` itself across three
branches to three distinct record types.

### When the guard set is NOT a type — two-sided approximation

Some pattern+guard pairs match a value set that **no type can denote** (e.g. "all maps of
size 2"). They bracket it with two types:

- **Potentially accepted type** `⟦pg⟧` (the paper's `〚 pg 〛`) — *over*-approximation,
  the smallest type ⊇ the match set. Used for **exhaustiveness** (and to be lenient about
  what *might* match). For `map_size(x) == 2` this is `map()`.
- **Surely accepted type** `⟦pg⟧` (lower bracket) — *under*-approximation, the largest
  type ⊆ the match set. Used for **redundancy** and to safely narrow inside the body.
  For `is_list(x)` it is `list()` (exact: `b = true`).

> "we approximate the set of all values that match a pattern-guard pair by two types: the
> smallest type larger than it, and the largest type smaller than it." (§4.2)

### The `case` typing rule (§4.2)

Given `case e do (pi gi → ei)`, with `e : t`, the type reaching branch `ei` is

```
ti = (t ∧ 〚pi gi〛_potential) \ ⋃_{j<i} 〚pj gj〛_surely
```

i.e. values producible by `e`, that *may* match `pi gi`, minus those *surely* caught by
an earlier branch (negative info from prior clauses — this is how **clause order** is
honored). Branch `ei` is typed only if `ti ≰ O` (non-empty; else it's redundant), under
environment `ti / pi`. Exhaustiveness side condition: `t ≤ ⋃_i 〚pi gi〛_potential`. The
rule is named `(caseΩ)` — the `Ω` means it may emit a runtime-exhaustiveness **warning**
because the union of *potential* types is an over-approximation; if the stronger
`t ≤ ⋃_i 〚pi gi〛_surely` also holds, no warning.

The full rule splits each `gi` into OR-clauses (`ti1…timi`), scanning **left to right**
to respect Elixir's short-circuit evaluation and possible guard *failures* (a later
conjunct is analyzed only in environments where the earlier ones succeeded — e.g.
`map_size(x)` examined only where `is_map(x)` held; a clause is typed only if preceding
clauses *may not fail*). An auxiliary judgment
`Γ; t ⊢ (pi gi) ⇝ (sij, bij)` emits, per OR-clause, a type `sij` plus a boolean `bij`
flagging whether `sij` is *exact* (→ surely type) or merely potential.

This guard analysis also yields **exhaustiveness** and **redundancy** checking that
report the *precise missing/dead type* (e.g. "no implementation for values of type
`%{output: :error, message: {:delay, integer()}}`").

---

## 5. Functions, arity, overloading

### Multi-clause functions = intersection of arrows

A function with several clauses (or several `$`-specs) is typed by the **intersection**
of its per-clause arrows. The running example: a `negate` over ints and bools is *not*
adequately typed by the union arrow `(integer() or boolean()) -> (integer() or
boolean())` (too imprecise — can't conclude int-in ⇒ int-out, breaks `subtract`), but
*is* by the intersection:

```
(integer() -> integer()) and (boolean() -> boolean())
```

> "an intersection type … specifies that `negate` has both type `integer()->integer()`
> … and type `boolean()->boolean()`." (§2.1)

Key subtlety — an arrow-intersection is **not** tied to having multiple clauses:
`negate_alt` with a *single* clause and an internal `if` also has the intersection type.
The intersection is a property of the *function value*, not its syntax. And the
intersection arrow is a *strict subtype* of the union arrow (the constant `fn x -> 42
end` inhabits the union but not the intersection).

### Input-dependent return types

This is precisely what the intersection-of-arrows buys: different input types ⇒ different
output types, statically. With `(integer()->integer()) and (boolean()->boolean())` the
checker proves int⇒int. A default clause adds more arrows; `negate` with an `x -> x`
fallthrough gets, via bounded quantification:

```
(integer() -> integer()) and (true -> false) and (false -> true)
  and (a -> a) when a: not(integer() or boolean())
```

Negation of clauses' domains models the fallthrough: the last clause's domain is "all
inputs minus those caught above," letting the system both order-sensitively type clauses
*and* check exhaustiveness. (The paper notes this beats eqWAlizer, which ignores clause
order and forces arguments to match a unique clause; §6.)

### Arity is first-class (the CDuce extension)

CDuce/semantic-subtyping treats all functions as unary (n-ary = unary-on-a-tuple), so it
*cannot* express "all binary functions": `{none(),none()} -> term()` collapses to
`none() -> term()` because a product with an empty factor is empty. Elixir needs arity
(it has `is_function(f, 2)`), so they add **arity-indexed arrow syntax**
`(t1,…,tn) -> t` and **rework the set-theoretic interpretation of function spaces** so
that subtyping over multi-arity functions is again a set-containment problem with a
decision algorithm (Appendix A.1 / companion [9]). Then "all binary functions" =
`(none(),none()) -> term()`. Note `none() -> term()` is the type of *all* unary
functions; the top of unary functions is **not** `term() -> term()` (only *total*
functions are safely applicable to every `term()`).

Polymorphism: first-order parametric polymorphism with **local type inference** and
bounded quantification via postfix `when a: <upper-bound>` (e.g.
`([a],(a->b)) -> [b] when a: term(), b: term()` for `map`). Lower bounds and exact
upper bounds are *encodable* via unions/intersections but get sugar because they're
common.

---

## 6. Maps and records, unified (brief)

One map type subsumes both records (fixed known keys) and dictionaries (computed keys),
§3.3. A map type is a set of **key-type ⇒ value-type** entries each marked
`required(...)` or `optional(...)`:

```
%{required(:age) => integer(), optional(term()) => term()}   # == %{age: integer(), ...}
```

- Singleton keys default to **required**; the `...` ("open map") sugar =
  `optional(term()) => term()`.
- `map.key` (record access) is **ill-typed unless the key is statically known present**
  — this *eliminates `KeyError` statically*. `m[key]` (dictionary access) is total,
  returning `value-type or nil` when the key may be absent (and just `value-type` when
  the checker proves presence). `Map.fetch!` is ill-typed only if the key is *always*
  absent.
- Non-singleton key **domains** must come from a fixed set of basic types
  (`integer()`, `atom()`, …) and **must not overlap**; they must be `optional` (a
  required infinite domain would demand infinitely many keys). Record and dictionary
  entries can be mixed in one type; singleton keys take precedence over domain keys.
- **Key deletion** is typed by pointing an optional key at `none()`:
  `%{optional(:foo) => none(), ...}` (the key must be absent, since a present key would
  need a value in the empty type). Update `%{m | k => v}` requires `k` present.
- Structs = closed records with a mandatory `:__struct__` atom field.
- The set-theoretic algebra (union factorization, negation) applies to map types too. A
  detailed contrast with Flow/TypeScript/Luau record types is in companion [8, §5].

---

## 7. Implementer pitfalls / soundness warnings the paper raises

1. **`dynamic()` is not `term()`, and subsumption is unsound for it** (footnote 12,
   §3.4). Do **not** widen a known type to `dynamic()` and rely on subsumption. The
   correct mechanic is *materialization of the surrounding type* (the arrow materializes
   to `integer()->dynamic()`), not "an integer is a dynamic." Conflating the two — the
   naive "dynamic = top" model — is the classic unsoundness.

2. **Do not bolt on a separate Siek–Taha consistency relation.** Derive consistent
   subtyping from **precision `≼` (materialization) composed with ordinary subtyping `≤`
   on non-gradual types** (`s' ≼ s1 ≤ s2 ≽ s`), following Lanvin [30] / [11]. A standalone
   consistency relation re-implements, less coherently, what set-theoretic precision
   already gives — and tends to mishandle the connectives.

3. **Negation interacting with `dynamic()` is the danger zone.** `?` appears under
   negation (`¬?`), and they explicitly need a *semantic* precision relation that
   respects equivalences, giving e.g. `¬? ≼ ?` (footnote 14). A purely syntactic
   "replace `?` by a type" precision would get negation/equivalence wrong. (This matches
   the well-known result that careless gradual + negation is a soundness hole — the
   reason their `≼` is semantic, not syntactic.)

4. **Soundness without casts requires modeling existing runtime checks.** Textbook sound
   gradual typing inserts casts and changes compilation; the explicit requirement here is
   that **types must not alter Elixir's compilation** (§5). The price of that promise is
   the **strong/weak arrow** machinery: you may only refine a `?` to a precise codomain
   (`? ∧ t`) when the function *provably* performs the check (its body, or a BEAM
   primitive). Refining `?` after a *weak* function would be unsound — the value really
   can be off-codomain. This is the "dynamic = the set of values the runtime checks"
   principle made operational.

5. **The intersection-with-`?` (`t ∧ ?`) is a deliberate leniency lever with a cost.**
   It maximizes typability of legacy code (lets a strong result flow into strict
   subtypes) but *defers* error detection: a wrong use that would be caught with a precise
   non-gradual type slips through and only fails at runtime (the `subtract`/`negate`
   walkthrough, §3.4). They flag it as probably opt-in.

6. **Clause order matters and is handled via negation.** Typing later clauses must
   subtract the *surely accepted* types of earlier ones; ignoring order (as eqWAlizer
   does) loses precision and can mis-type overlapping clauses (§6).

7. **Guards can fail, not just return false** — left-to-right analysis must only type a
   conjunct in environments where prior conjuncts succeeded; otherwise you wrongly merge
   environments (the `is_map`/`map_size`/`is_list` example, §3.2/§4.2).

8. **Not every value set is a type.** Guards like `map_size(x)==2` force the
   over/under-approximation discipline; conflating "matched set" with "a type" without the
   two-sided bracket breaks exhaustiveness/redundancy soundness (§4.2).

9. **Semantic (not syntactic) subtyping is load-bearing.** The product-union
   factorization and arrow equivalences they rely on (and that distinguish them from
   TypeScript/Flow/Typed Racket) only hold under the set-theoretic *model*; a syntactic
   subtype check will reject programs they accept (`apply`/`t()`≡`s()`, §2.2).

---

## 8. Deliberately deferred / future work

- **Type reconstruction (full inference)** beyond guard-driven inference is deferred —
  priority is *checking* most idioms first; reconstruction is multi-pass and expensive
  and needs a cost/benefit study (§5, §7). Untyped parameters default to `dynamic()`
  rather than being inferred (milestone 3).
- **Full occurrence typing à la [12]/[13] extended to polymorphic types** — not yet:
  e.g. they cannot currently reconstruct the precise `filter`/`l_or` types that require
  narrowing a variable used as an argument to an intersection-typed function (§7). Some
  non-variable test narrowing has "exceptions" today (§3.2).
- **Row polymorphism for maps** — needed to type field-update/delete-returning functions
  polymorphically (`%{a} -> %{optional(:foo)=>none(), a}`); "extending semantic subtyping
  with row polymorphism is an **open problem**, and we are currently working on it" (§7).
- **Lifting the fixed key-domain restriction** — allowing programmer-declared (and
  eventually inferred) finite partitions of key types instead of a predefined set (§7).
- **Concurrency / message-passing types** — typing `receive`, process interfaces from
  the *potentially accepted* type of receive patterns, ultimately **behavioral / mailbox
  types** [19,22] — "an obvious next step," longer-term (§7).
- **Behaviours and first-class modules** — typing modules/behaviours needs
  existential/opaque types (packaged modules [44,42] or Julia-style bounded existentials
  [52,18]); abstract types currently approximated by `term()`; even a `dynamic()`-based
  simulation needs care (§7, Appendix C with GenServer mock-up).
- **Programmer-visible strong-arrow annotations** for higher-order parameters — strong
  types are internal-only today (§3.4).
- **Alternative `?`-propagation disciplines** — e.g. propagate `?` whenever an
  application's types have *any* `?` component (more lenient, less precise) — to be tested
  on real code bases (§4.3).
- **Removing the "no compilation change" restriction later** — to optionally drive
  runtime checks / performance from types (§5).
- **The full proofs and algorithms** for arity, guards, and gradual typing are *not in
  this paper* — they live in companion papers [8] (maps) and [9] (arity/guards/gradual).

**Rollout plan (§7):** (1) types internal-only, no user syntax — catch obvious mistakes
from patterns/guards; (2) annotations on **structs** only (closed records); (3)
`$`-prefixed function annotations, untyped params ⇒ `dynamic()`, little/no
reconstruction. Prototype: `typex.fly.dev`, initially using the CDuce type library,
being reimplemented in Elixir for the compiler.
