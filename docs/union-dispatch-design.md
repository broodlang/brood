# Union dispatch positions + typed multimethods — design

> Status: **designed, not built.** Written 2026-08-28. The roadmap entry
> ("Union dispatch positions — and why the type system is the easy half") points here.
> Nothing below is implemented; this is the record of the decisions so they are not
> re-derived, and of the four things checked empirically rather than assumed.

## The ask

Dispatch on a *union* in one argument position — `[usd (or :int :float)]` — and have the
type checker still derive the result. Today that is a clean expansion error: *"each position
must be a record name or a built-in id keyword"*.

## Decision 1 — typed multimethods, not multi-argument abilities

The first instinct was "make `defability` multi-argument, then unions fall out". That is the
wrong shape. An `impl` is keyed on **one** id: an ability says *"this type implements this
interface"*. Multi-argument dispatch is a **relation between types**, which is what `defmulti`
already is — that is why Brood has both forms, and the split is worth keeping.

So the move is the other direction: **give `defmulti` a declared return type**.

```lisp
(defmulti compare-to :antisymmetric :-> int)
```

Three things make this the cheap path, all verified:

- `defmulti` already has per-argument id dispatch, `:default`, and the
  `:commutative`/`:antisymmetric` algebra.
- The checker **already tracks multimethods** — `types/check/protocol.rs` calls itself
  "the `defmulti` analogue of `check_ability_calls`" and already knows which identity-tuples
  each multimethod covers, warning on a call no method handles. Only the *return* is missing.
- `defmulti`'s opts parsing is strict (an unknown algebra keyword is an expansion error), so
  extending it cannot silently accept nonsense.

**Why a declared return is what makes this work at all.** For abilities, the return comes from
the op declaration and `check_impl_returns` verifies every impl body against it — which is why
`types/check/infer.rs` can say *"the declared return is the only static handle… a contract, not
a guess"*. Inference never joins return types across impls, so **which** method runs is
irrelevant to the result type. That is exactly the property that makes union positions free
for the checker, and multimethods need the same declaration to get it.

## Decision 2 — the specificity rule

Today dispatch is: exact identity-tuple, else `:default`, else a loud `%no-method`. There is
**no middle tier and no possibility of two methods matching**. A union introduces both, so a
rule is required.

**Dispatch key.** A tuple of ids `[id₁ … idₙ]`, one per argument, from `%identity-of`.

**Pattern.** A vector of positions; each position is a *set of ids* — a bare id is the
singleton set, `(or a b)` is `{a, b}`. Plus the whole-pattern `:default`.

**Matching.** Pattern `P` matches key `K` iff `K[i] ∈ P[i]` for every position `i`.

**Specificity.** `P ⊑ Q` iff `P[i] ⊆ Q[i]` for every `i`. This is a *partial* order — the
subset ordering, position-wise.

**Resolution.**

1. Collect every matching pattern.
2. None → `:default`; if there is none, the existing loud `%no-method`. *(unchanged)*
3. Exactly one → use it.
4. Several → the unique `⊑`-minimum. **If there is no unique minimum, raise**, naming both
   patterns and the key.

**Today's behaviour is preserved exactly, which is the point.** An exact tuple is all
singletons, so it is `⊑` every union that contains it — exact always wins, `:default` is
widest. No existing program changes meaning.

**The ambiguity that must raise:**

```lisp
(defmethod f [usd (or :int :float)] …)
(defmethod f [(or usd eur) :int]    …)
(f (usd 1) 2)   ; key [:m/usd :int] matches both
                ; position 1: {usd} ⊂ {usd,eur}   → first is narrower
                ; position 2: {int,float} ⊃ {int} → second is narrower
                ; incomparable ⇒ error, not a pick
```

Raising rather than picking follows ADR-179's existing stance: *"an EXACT method, then a
`:default` method, else a loud `%no-method` error — **never a nearest guess**"*. A
"most-recently-defined wins" or left-to-right tiebreak would make dispatch depend on load
order, which is exactly the class of bug nominal dispatch was chosen to avoid.

**Detect at registration, not only at the call.** Two patterns that overlap without one being
more specific are a latent error the moment both are registered — no call is needed to know
that. Check pairwise at registration (`n` is small per multimethod) and raise there, with the
call-site check kept as the backstop for methods registered across modules.

## Decision 3 — derived methods do not clash (already true)

`:commutative` derives each off-diagonal method's mirror. The question was whether a derived
method could collide with a hand-written one. **It cannot, and this already works** — verified:

```lisp
(defmulti m :commutative)
(defmethod m [usd :int] (a b) :upper)
(defmethod m [:int usd] (a b) :lower-explicit)
(m (usd 1) 2) ; => :upper
(m 2 (usd 1)) ; => :lower-explicit   ← the explicit one, not the derived mirror
```

So the rule to preserve is: **an explicitly authored method wins; derivation over an existing
key is a no-op, not an error.** Filling in the lower triangle yourself is *more* information,
not a conflict.

Extending it to unions: the mirror of `[usd (or :int :float)]` is `[(or :int :float) usd]` —
mechanical. The one new case is two *derived* patterns that are mutually ambiguous; that is
Decision 2's rule applied at derivation time, and raises the same way.

## Decision 4 — a union is a set of ids, not a type

`(or …)` in a dispatch position **borrows the type language's spelling, not its semantics**.
Every member must resolve at expansion time to a literal id: a record name (→ its
`:module/name` keyword) or a built-in kind keyword. Nested `(or …)` flattens. Anything else —
a structural type like `(record :x int)`, a type variable, `(and …)`, `(not …)` — is an
expansion error.

The reason is that dispatch is **nominal** (ADR-177/179): the key is an identity keyword from
`%identity-of`, and a structural type has no id to match against. Borrowing the spelling means
users do not learn a second syntax for "one of these"; borrowing the semantics would quietly
turn nominal dispatch into structural dispatch.

## Decision 5 — bignum shares `:int`, and that is correct

`(%identity-of (* 99999999999999999999 2))` is `:int`, not a separate id. Checked: this
**agrees with the type system**, which has no `BigInt` tag at all — `Tag` is
`Nil Bool Int Float … Decimal Set`, so the lattice already folds bignum into `int`. Since ints
auto-promote to bignum on overflow, i64-vs-bignum is a representation detail, and dispatch
treating them as one integer type is the consistent choice, not a compromise.

**The one negative:** a bignum-specific fast path cannot be selected by dispatch — it needs a
predicate branch inside the method. Judged acceptable: a caller should not have to know which
representation their integer landed in, and the alternative (a `:bigint` id) would make the
dispatch vocabulary disagree with the type vocabulary, which is a worse seam.

## What is left to build

1. `defmulti` opts accept `:-> RET`; `%register-multi` stores it.
2. `check_method_returns` — the `check_impl_returns` analogue. **Must land with (1)**, or the
   declared return is an unchecked assertion and the soundness argument above collapses.
3. `infer.rs` consults the declared return for a multimethod call, as it already does for an
   ability op's `:-> RET`.
4. Union positions in `defmethod`, with Decision 4's expansion-time reduction to id sets.
5. Decision 2's specificity resolution + registration-time ambiguity detection.

(1)–(3) are independently useful — they make multimethod calls typeable at all, which is what
a `Comparable`-style ability over the numeric tower needs — and can ship before (4)–(5).

## Why this is worth building (the bar)

Per the roadmap's test for a language addition, a union position **fails** as a convenience:
two methods already express one union. It **passes** for the numeric tower, where a binary op
over `int`/`float`/`decimal`/`ratio` needs 16 methods that `:commutative` only halves. That is
the concrete need — without it, a typed `Comparable` over the tower is not practically
writable, and KI-75 showed the tower's comparison semantics are load-bearing.

---

## Follow-on decision (2026-08-28): cross-type comparison should not be a kernel default

Raised while designing the above: `(compare 1 1.0)` and `(< 1 1.5)` coerce **silently**, and
that coercion is *policy* living in Rust (`core/heap/equality.rs`). Nothing in Brood declares
it, and no user can see, change, or opt out of it. Against three things the language already
does, that is the odd one out:

| | cross-type behaviour |
|---|---|
| `(= 1 1.0)` | **strict** — false, deliberately (`docs/language.md`, `docs/spec.md`) |
| `(+ (usd 1) (usd 2))` with no method | **raises** — `num/add` has no `:default` (ADR-179, "never a nearest guess") |
| `(compare 1 1.0)` | silently coerces to `0` |
| `(< 1 1.5)` | silently coerces |

Records get strictness and a visible extension point; the numeric tower gets a hidden kernel
default. **Decision: cross-type comparison raises unless the program supplies a resolver** —
either a declared method for the type pair, or a comparator function at the call site (which
`(sort less? coll)` already provides).

### The blast radius, measured rather than assumed

`value_cmp`'s cross-type numeric arms were instrumented and the whole in-language suite run
(216 files):

| | cross-type comparisons |
|---|---|
| prelude boot | **0** |
| a realistic program (`json_test`) | **0** |
| whole suite | **14** |
| …of which `comparison_test.blsp` (written *to test* cross-type comparison) | 13 |
| the one real site — `(math/max 1 2.5 2)` in `math_test` | 1 |

So the convenience being defended is very close to theoretical. **And strictness makes the
DEFAULT path cheaper, not dearer:** the fast path is already same-type, and the coercion arms
become an error path. Dispatch cost lands only on programs that install a resolver.

### Compile-time resolution (the intended optimisation)

Most comparison sites can be decided before runtime, and the machinery exists:

- the JIT already specialises arithmetic on observed operand types, and `resolve_prim` lowers
  `math/*` wrappers to their `PrimOp` by call-site name — a same-type comparison should lower
  to the native compare with no dispatch at all;
- the checker frequently knows **both** operand types statically, so a mixed comparison with
  no resolver can be a **checker warning** rather than a runtime raise — strictly better than
  either the old silent coercion or a late error.

### Arithmetic follows the same rule — and the goal is DECLARED, not strict

`(+ 1 1.5)` is `2.5` today: the int is promoted by float contagion. `(+ 1 1M)` is `2M`. Same
shape as comparison — a coercion rule living in Rust that no Brood program declares. Note what
is *not* affected: `(/ 1 2)` is `1/2` (a ratio — same-type division is already exact), and
i64 → bignum on overflow is promotion *within* the integer type, not a cross-type coercion.

Measured the same way (`num_to_f64` instrumented for operator-driven coercion only, excluding
the explicit `->float`), across the whole suite:

| | contagions |
|---|---|
| prelude boot | **0** |
| `math_test` | 5 |
| `ratio_test` / `jit_bool_arith_test` / `decimal_test` | 2 each |
| `try_catch_test` / `doc_examples_test` | 1 each |
| **total** | **13** |

Thirteen, in 40k lines of stdlib and 216 test files. Implicit numeric coercion looks
indispensable and is empirically almost unused — the same result as comparison (14).

**The objective is not strictness; it is that the coercion is DECLARED.** Ergonomics matter,
and a language that makes ordinary arithmetic strenuous has traded one bad default for
another. So the deliverable is the *mechanism* — a coercion/comparison that a program can
read, override, and extend — and the default becomes a knob that is **chosen and written
down** rather than inherited from `numeric.rs`.

The shape that satisfies both: **`std` ships the numeric-tower coercions as declared methods,
in Brood.** Ordinary code then behaves exactly as it does today, and:

- the rule is visible where a reader can find it, not buried in a Rust match arm;
- a user can override it for their own type pair, or define one the tower does not cover;
- opting out is possible and is a decision, not a fork of the kernel;
- **strict-by-default becomes one setting** rather than a rewrite — it is "do not ship the
  tower declarations", which the 13/14 measurements say costs almost nothing to try.

That ordering also de-risks it: build the mechanism, keep today's behaviour, then decide the
default on evidence rather than in advance.
