# Type system — status & what's left

**Revised 2026-08-27**, after the work the day's audit ranked (ADR-259..263); previous
wrap-up 2026-07-30. Where the type system actually
stands against its goal — a **set-theoretic, gradual, advisory** system in the Castagna
line (see [research/set-theoretic-types-in-brood.md](research/set-theoretic-types-in-brood.md)).
For the model and the compatibility contract see [types.md](types.md); for the "why" of each
piece, the ADRs cited below in [decisions.md](decisions.md).

The invariant that governs everything: **the checker is advisory and sound — it warns only
on a *provable* misuse and never false-positives.** Every feature below landed against a hard
gate: `nest check std/**/*.blsp tests/**/*.blsp` stays at zero warnings, and the
`types::check` unit suite stays green.

---

## Verdict

**The three layers the 2026-08-27 audit separated — the lattice, the inference in front of
it, and the annotation surface that feeds it — have all moved.** What that audit measured as
missing is now measured as present; what remains is listed under
[What's left](#whats-left), and it is smaller and more specific than it was.

| Layer | State |
|---|---|
| **The lattice** — values-as-sets, semantic subtyping, ∪/∩/¬/∖, gradual `dynamic(bound)` | A union now **keeps its terms** (ADR-262), so a union of two structured types is exact instead of widening to a bare tag, and a record **says what a value is not** (ADR-264) — closed by default, openness modelled as the type of the undeclared keys — which is what makes that union usable rather than merely representable. Complements are sayable (`(not T)`, ADR-263). The relations are now checked *against each other* over a type corpus (`disjointness_agrees_with_intersection` and friends), which found three defects a per-case test had not. Still approximate in two documented places: the complement of a *refined* term, and subtyping's incompleteness across terms — both the safe direction. |
| **Inference** — how a function's domain and range are derived without annotations | A parameter's type is now its **domain** (ADR-261): a guarded use is credited *within its guard* and the alternatives union, so branch shapes, `match` patterns, head destructuring, multi-arm functions and `:when` clause guards all constrain callers with no annotation. |
| **The annotation surface** — `sig`, and the checker's reach over program text | `sig` **fails closed** (ADR-259) and the *definition* owns the arity. The walk's totality is now gated (ADR-260), and that gate immediately found the next instance of the KI-67/KI-70 class (quasiquote escapes). |

---

## Measured, not asserted

The probe corpus below ran against `brood --check`. Everything in **Caught** was verified
after the change; the four entries under **Still missed** are the honest residue.

### Caught ✅

Everything the audit listed, plus:

```lisp
;; the domain rule (ADR-261) — no annotation anywhere
(defn f (x) (if (string? x) (string/length x) (+ x 1)))   (f :kw)   ; neither branch admits it
(defn d ([a b]) (+ a b))                                  (d 5)     ; head destructuring
(defn m (x) (match x ((:ok v) v) ((:error e) e)))         (m 5)     ; no clause matches
(defn g ((x) :when (string? x) …) ((x) :when (int? x) …)) (g :kw)   ; no clause accepts
(defn h ((x) (string/length x)) ((x y) (+ x y)))          (h 5)     ; arm 1 wants a string

;; unions of structured types (ADR-262)
(sig t ((or (tuple int) (tuple string)) -> any))       (t [true])
(sig r ((or (record :a int) (record :b int)) -> any))  (r {:z 1})
(sig v ((or (vector int) (vector string)) -> any))     (v [true])

;; the surface itself (ADR-259)
(sig q1 (strng -> int))                          ; unknown type
(sig q2 ((tupel int) -> int))                    ; unknown constructor
(sig q3 (int -> int)) (defn q3 (a b) …)          ; sig contradicts the definition
(sig q4 (int -> int))                            ; annotates nothing
(defn n1 (x y) x) (defn n2 () (n1 1))            ; same-file arity
(sig g ((int -> int) -> int)) (g string/length)  ; arrow-typed parameter
(defmacro m (x) `(a ~(zzz x)))                   ; a quasiquote escape is code
```

### Still missed ❌

```lisp
;; 1. a refinement accessor reports nothing for a union, so a *field* of a
;;    tagged-union value is not resolved — the relations improved, not the lookups
(sig f ((or (record :ok int) (record :error string)) -> int))
(defn f (r) (string/length (get r :ok)))          ; silent

;; 2. the complement of a refined type widens to its tag
(sig f ((not (tuple int)) -> any))  (f [1])       ; silent

;; 3. a callback's *result* is never checked (an inferred return over-approximates)
(sig g ((int -> string) -> int))  (defn h (n) (+ n 1))  (g h)   ; silent

;; 4. subtyping across terms is sound, not complete: a value covered jointly by
;;    two alternatives but by neither alone reads as "not a subtype" — it defers
```

## What's left

Eight of the nine items the audit ranked shipped the same day (ADR-259..263); what follows is
what they left behind, plus the items that were deferred on ADR-011 grounds and still are.

| # | Item | Why it is left | Cost |
|---|---|---|---|
| ~~1~~ | ~~A field lookup on a tagged union~~ | **Shipped** (ADR-264): records are closed by default, with `&open` as the marked case and openness modelled as the type of the undeclared keys, so `(get r :ok)` over `{ok: int} \| {error: string}` resolves to `int \| nil` and the two arms are provably disjoint | ✅ |
| 2 | **The complement of a refined term** — `(not (tuple int))` widens to `vector` | Needs negative structural atoms (the full BDD), i.e. terms that say "not this shape" — a second representation change, with emptiness-checking to match | Large |
| 3 | **A callback's result** is never checked | An inferred return over-approximates, so comparing results false-positives at every call site; needs a "this return is precise" distinction the sig sources do not carry today | Medium |
| 4 | **Subtyping across terms is incomplete** — a value covered jointly by two alternatives but by neither alone defers | The complete rule needs a distributivity/emptiness decision procedure; the incompleteness is in the safe direction (it defers, never warns) | Large |
| 5 | **Return-type dispatch** — selecting an impl by expected return | Needs bidirectional inference. The long-standing open item in [protocol-dispatch-design.md](protocol-dispatch-design.md) | Large |
| 6 | **Qualified cross-module ability type names** (`mod/Ability` in a `sig`) | Ability type names resolve by bare name; this is also what makes ADR-259's capitalised-name silence necessary | Small |
| 7 | **Tier-2 monomorphization** — devirtualizing an *inferred-variable* op call | The real hot-loop win, and the miscompile surface; needs the checker→compiler channel plus whole-fleet validation. See [ability-monomorphization.md](ability-monomorphization.md) | Large |
| 8 | **Runtime contracts for ability ops** under a `BROOD_CONTRACTS`-style flag | Checker-only today, matching `sig`'s default (ADR-180 deferred item c) | Small |
| 9 | **`sig` adoption across std** — 34 declarations over 2828 `defn`s | Every one now buys more than it did: a declared sig is what the reversed-args gate (KI-71) reads, and inference is checked against it (ADR-259) | Ongoing |

**Precision residues, still sound to leave.** The *merely-wider* residue — a body typed
exactly `number` declared `int` (e.g. `(/ x 2)`) — needs occurrence/range analysis to pin and
would false-positive if flagged (ADR-011). Element-typed `seqable` is still unrefined; a
genuine element-typed seqable needs extending the `elem` refinement beyond `Pair|Vector`.

---

## Inventory — what exists

### The lattice (`types/mod.rs`)
Set-theoretic + gradual (ADR-023/024). A `Ty` is a **union of terms** (ADR-262): one term is
a tag bitset plus at most one refinement per slot — function **arrows** and **overloads**
(arrow intersections), sequence **element types**, **`(map K V)`**, **record shapes**,
**literal singletons** (keyword/int/bool/string), **tuples** — and a union that cannot merge
into one term keeps up to four. The named unions `number` / `list` / `seqable` and the
complement `(not T)` (ADR-263) round out the grammar. `GradualTy { bound, dynamic }` is the
gradual valve — `dynamic(T)`, with consistent subtyping derived from set inclusion. It is
used at exactly one site (the `(def x …)` assignment check); the rest of the checker's
vocabulary is `Option<Ty>` (known / unknown), which is why there is no strict mode and no
strong arrows.

### Signature sources (`types/check/sigs.rs`)
Simplest-first: **primitives** (every `NativeFn` carries a `Sig` — 382 of them, only 14
fully `any`), **curated** stdlib (a hand-vetted table for variadic/HOF closures),
**declared** (`sig`/`sig!` — 34 in `std/`, 23 in `tests/`, each now validated against its
definition, ADR-259), and **inference**: a parameter's **domain** over the body's possible
executions (ADR-261 — a guarded use credited within its guard, alternatives unioned, a
`match` failure branch contributing ⊥), one signature **per arm** of a multi-arm or
`:when`-guarded definition, returns from the body tail unioned across branches,
self-recursion contributing ⊥, and same-file `defn`s inferred from their forms over a
bounded leaf-up fixpoint (ADR-188/190).

### Typed abilities (ADR-180/181/185/186/187/192/193)
Op specs carry types; returns flow into inference at every call site; impl bodies are graded
against them. Any ability name is a type (sealed → the union of its members' record shapes;
open → `any`). Missing-impl warnings, sealed exhaustiveness, per-module op-name uniqueness,
record patterns in `match` with exhaustiveness, provided op bodies, ability bounds in a `sig`,
and `:requires` super-ability conformance.

### Devirtualization (ADR-182) — `BROOD_MONO`, off by default
Tier 1 rewrites an op call with a literal or direct-constructor first arg to a direct impl
call. Flag-off is provably inert. Tier 2 (inferred-variable dispatch — the real hot-loop win)
is unstarted; see [ability-monomorphization.md](ability-monomorphization.md).

### Reach
`nest check` / `nest test` / `nest run` / `brood <file>` / `brood --check`; MCP `load`/`check`;
LSP diagnostics + hover; REPL advisory warnings; per-file require-reachability (ADR-189);
`make check-corpora` runs a static pass over `examples/`, `stress/` and `breakage/`, which
were previously gated only by *running* them. The walk's **totality** is itself gated
(ADR-260): every `SPECIAL_HEAD` entry and every container literal has a planted-name case in
`REACH_CASES`, in both the whole-file and expanded-fragment walks, and a companion test
requires a new special form to declare what its body is for.

---

## Where things live

| Concern | File |
|---|---|
| Lattice (`Ty`, ops, named unions), `GradualTy`, `Sig` | `crates/lisp/src/types/mod.rs`, `types/sig.rs` |
| Checker entry + passes (`check_file`) | `crates/lisp/src/types/check.rs` |
| The walk (special-form handling, call checks, arity) | `crates/lisp/src/types/check/walk.rs` |
| Signature sources + inference | `crates/lisp/src/types/check/sigs.rs`, `infer.rs` |
| Guard narrowing / occurrence typing | `crates/lisp/src/types/check/guards.rs`, `guard_effects.rs` |
| Type-annotation grammar + its validation (`sig`, `record`, `tuple`, `not`, ability-as-type) | `crates/lisp/src/types/check/annot.rs` |
| The reach gate (`REACH_CASES`) and the lattice/checker tests | `crates/lisp/src/types/check/tests.rs`, `types/tests.rs` |
| Ability/multimethod checks | `crates/lisp/src/types/check/protocol.rs` |
| Record-pattern exhaustiveness | `crates/lisp/src/types/check/exhaustive.rs` |
| Devirtualization (`BROOD_MONO`) | `crates/lisp/src/eval/compile/inline.rs` |
| REPL advisory check | `std/tool/repl.blsp` |
| LSP hover / diagnostics | `crates/lsp/src/{hover,main}.rs` |

ADRs: **023/024** the lattice + gradual · **078** structured refinements · **105/117/120**
literal singletons · **116** overloads · **127** `&optional` · **128** tuples · **180** typed
op returns/params · **181** sealed ability as a type · **182** mono
devirtualization · **185** provided op bodies · **186** any ability name is a type · **187**
record patterns + exhaustiveness · **188** same-file inference · **189** per-file
require-reachability (KI-17) · **190** occurrence typing · **191** staged call head (KI-19) ·
**192** ability bounds · **193** super-abilities · **226** clause guards · **259** a
declaration that cannot be read is reported · **260** the walk's totality is gated · **261**
parameter domains · **262** a union keeps its terms · **263** `(not T)`.

---

## Seeing it while you edit (2026-08-28)

The checker's answer is now visible in the editor, which is where it is worth
having. `types::check::file_signatures` exposes what `check_file` already computes —
the effective signature of every function a *buffer* defines, inferred from forms
without loading it (ADR-188/190/261), which is the question `hover` structurally
cannot answer because it reads the loaded image.

- **An inlay hint** after each `defn`'s parameter list, showing the type the checker
  inferred. Quiet by construction: nothing for a function that already carries a
  `(sig …)`, nothing for an uninformative `(any …) -> any`, nothing it declined to
  infer, and only the informative half (`→ T` when the parameters are unknown).
- **A "Declare signature" code action** that writes that signature into the file as a
  real `(sig …)`. `Ty::to_source` renders it — the inverse of the annotation parser,
  round-trip tested over the whole corpus — and declines rather than approximating
  when a type has no faithful spelling.

That pair is also the answer to this document's longest-standing backlog item, **`sig`
adoption**: 34 declarations over 2828 definitions is not a problem you fix by asking
people to type more, and a hint you can accept with one keystroke is the cheapest path
from "the checker knows" to "the file says".

---

## Adoption in bulk, and what was invisible while adopting (2026-08-28)

The editor pair above is the per-function path. Three things followed from trying it
at the scale of the standard library.

**`nest check --suggest-sigs`** — the bulk counterpart. It prints the `(sig …)` the
checker would write for every function that lacks one, grouped by file, and changes
nothing. Adopting one is *sound*: an inferred parameter domain over-approximates the
real one (ADR-261, asserted by the soundness oracle), so a call the declaration
rejects would have failed anyway. It is still advice rather than a patch, because a
signature is documentation and deserves a reader. The mechanism is a new primitive,
`reflect/file-signatures`, returning `{:name :sig :declared? :informative?}` per
definition — `:informative?` decided on the *types*, since the rendered text cannot be
tested for it (`(string any -> any)` contains the text of the uninformative
`(any -> any)` and is worth declaring).

**A module-private function had no inferred signature at all.** `defn-` expands to
`(do (def name (fn …)) (%mark-private 'name))`, and every inference pass keyed on a
top-level `(def …)` saw no definition there — so a private function's *call sites went
unchecked*. That is most of a real module: 40 of `std/json.blsp`'s 42 definitions, and
precisely the internals where an argument-order slip lives (the KI-71 class). The
descent is deliberately narrow — only the privacy expansion, identified by its
`%mark-private` call — because opening every top-level `do` was tried and reverted
twice over: it typed the linear-map rewrite's generated temporary (flagging a branch of
the rewrite's own wrapper that cannot run) and it displaced the `:-> T` return that
`defability` declares for its ops. A gensym'd name is now never typed on its own
account, whatever encloses it.

Arming this across `std/` + `tests/` produced **zero** new warnings — the coverage
grew and the corpus stayed clean.

**Cross-term subtyping got its per-tag decomposition.** `is_subtype` over a union
required each term of the left to fit inside a *single* term of the right, which is
sound but incomplete — and the incompleteness costs a **false positive**, the one
unacceptable class. `int | vector<int>` is a single term (the union merged exactly)
and sits inside `int | vector<string> | vector<int>` only once you notice its two
halves land in different alternatives. A term is the disjoint union of its per-tag
projections, so placing each projection somewhere is both sound and strictly sharper.
Still incomplete where one *tag's* refinement is split across alternatives
(`vector<int|string>` against `vector<int> | vector<string>`); deciding that needs the
emptiness procedure a full negation type would bring.

The same projection fixed `Ty::to_source`, which dropped a refinement on any term
carrying tags beside it — `int | vector<int>` rendered as `(or int vector)`. Caught by
the round-trip test the moment the type entered the property corpus.

## The complement of a literal (2026-08-28)

`(not T)` (ADR-263) gave the lattice an exact complement for **tags**. It gave none for
**literals**: `¬:ok` widened to `any`, because a keyword domain is infinite and a literal
set could only be held positively. Bool was the one exception — a finite domain, so
`¬{false}` is `{true}`, which is what had made the truthiness guard biconditional.

That gap sat exactly where a set-theoretic system is supposed to earn its keep. The
**tagged-union dispatch** — the shape most Brood code branches on — refined only on the
true side, and the equality guard was marked `then_only` for precisely that reason.

A literal refinement is now `In(A)` or `Out(A)` — exactly these values, or anything but
these (ADR-268). `(or :ok :err) ∩ ¬:ok` is `:err`. The four literal slots carry a
`LitSet` instead of a bare set, the algebra is one rule per pair, and bool is normalised
back to positive so every rule may assume an `Out` set has an infinite complement.
Consumers outside the lattice read literals through `members()`, which reports `None` for
a negative set — the same conservative widening they already handled, so nothing outside
`types/mod.rs` changed.

The equality guard is biconditional **where the guard type is exact**: `(= tag :ok)`
narrows both branches, `(= m "x")` still narrows only the true one, because `of_value` has
no heap to read a string literal's bytes and yields the bare `string` tag. Rendering is
`(not :ok)` or `(and keyword (not :ok))`, both round-tripping through the parser, and the
runtime `type-matches?` agrees with the checker on all of them.

Zero new warnings across `std/` + `tests/`, with the property corpus extended to carry
negative atoms so every lattice law is asserted over them.

## The refinement-carrying rules, and a false positive on `assoc` (2026-08-28)

Closed records made the record *sinks* load-bearing: without them a closed record decays
to a flat `map` on its first update, and the idiom that builds one field at a time loses
its shape immediately. `assoc`/`dissoc` now carry the shape forward, and `keys`/`vals`
report the declared names and the union of the declared field types — for a **closed**
record only, since an open one may carry keys nothing declares.

Building those surfaced a real defect in the neighbouring rule (ADR-269). `(assoc m
:extra "text")` on a `(map keyword int)` was typed `(map keyword int)`, so reading the key
back gave `nil | int` and the checker **flagged correct code** — a false positive on the
operation everyone uses to build a map. The rule carried `K`/`V` forward unchanged on the
recorded grounds of "no false-positive risk either way", which was wrong in the direction
that matters: claiming a narrower type than reality is what manufactures one.

The lasting fix is the gate, not the rule. The soundness oracle checks **map and record
refinements** now — a map value's entries against `map_kv`, each declared field, and a
closed record's claim that no other key is present. A tags-only membership check passes
on any map-typed expression whatever its refinement says, which is why this survived an
oracle that had been running since the refinements were introduced. Sabotage-verified in
both directions.

## Two spellings, and a suggestion that could not be pasted (2026-08-28)

Declaring the first curated batch of signatures — the **KI-71 class**, a function whose
parameters have different concrete types, where a reversed call is accepted in silence —
turned up two defects in the machinery that offers them.

`(sig string/last-index-of (-> int))` was offered for a three-parameter function
(ADR-271). An inferred signature is a fact about types and says nothing about shape, so a
function whose parameters the checker could not type came out nullary; pasted in, Pass
2.85 rejects it as contradicting its `defn`. A captured signature is now reshaped to the
definition's parameter list, filling untyped slots with `any` — and it fills in rather than
overruling, since a multi-clause `defn` lowers to a variadic `fn` whose form-level arity
would discard what the clause inference knows.

`(or false true)` was not `bool` (ADR-270) — not merely rendered differently, but a
different `Ty`: unequal, unhashable together, and `bool <: (or false true)` came out
**false** for two identical sets. Literal slots are canonicalised now, so no operation can
produce the second spelling.

Twelve declarations landed: `string/char-at`, the six `text/*` rope operations, the three
`reflect/scan-form-*` scanners, `math/->fixed` and `bytes/at`. `(string/char-at 3 "abc")`
and `(text/insert r "text" 3)` are warnings now; the corpus stayed at zero. That is 12 of
890 the tool can already write — the rest is judgement per declaration, not archaeology.

## The callback position (2026-08-28)

Declaring the reversal-prone signatures showed what was still missing: `(seq/group-by
[1 2 3] f)` stayed silent, because a *callback* parameter typed `any`. Every ADR-261
domain rule reads a parameter's arguments — passed to a known callee, tested by a guard,
destructured by a pattern — and none read the head of a call, which is the position that
says the most about a function parameter.

A parameter in call-head position is now intersected with the callable type (ADR-272),
sound on the same footing as every other demand: `(g x)` only runs if `g` is callable.
Callable is `fn | native | keyword`, since a keyword is a function of a map in Brood while
maps, vectors and strings raise. `(each-of 5 [1 2 3])` is a warning now, and so is any
higher-order call with its arguments the wrong way round once the callback is inferred.

The propagation compounds with adoption: with `string/char-at` declared, `(defn indirect
(n s) (string/char-at s n))` infers `(int string -> string)` on its own.

## The four worries, closed (2026-08-28)

An audit of what was still weak found four things; three were real and one was a
misjudgement worth pinning.

**An arrow parameter was inert** (ADR-273). `(sig apply-it ((int -> string) -> any))`
declares a parameter's full signature, and the call inside the body is the only site that
can use it — but that path consulted only *global* signature sources, so `(f
"not-an-int")` went unchecked and `(f 1)` produced no type. Declaring an arrow changed no
outcome, which is a poor lesson to teach an author now that every callback parameter
infers as callable (ADR-272). A variable whose own type carries an arrow now describes the
call it heads, in both directions, consulted ahead of every global since a local shadows
one.

**The oracle could not reach `map<K, V>` or arrows.** Its expression facet types *closed*
expressions, and neither shape can arise without an annotation — so every `map_kv` rule and
every arrow rested on hand-written tests, and ADR-269 was a defect in exactly that gap. A
new facet types a body under a parameter given a type *through the annotation parser
itself*, then evaluates the same body with a value of that type bound and requires the
result to be a member of what the checker claimed. It catches ADR-269's `assoc` defect
automatically. Two further blind spots were closed alongside it: **literal sets**
(`contains_tag` passes `6` against `{5}`) and **tuple shapes** (length and positions, which
`elem_ty` does not describe). All sabotage-verified; none found a live defect, which is
what was true of the map rules until the day one of them wasn't.

**What a declaration catches was pinned, not changed.** A closed record catches a wrong
field name, a wrong field type, a missing required field and an extra one — the last two
only because ADR-264 made closedness provable. What it deliberately does *not* catch is an
argument whose type is a union that **might** be right: `(or int string)` against `string`
is silent, because argument checks fire on provable disjointness, and warning there would
fire on correct code whenever the checker knows less than the programmer. A union with no
`string` in it *is* flagged. I misjudged this line myself while writing a test, so it is a
regression test now rather than a belief.

**`fn` is one word** (ADR-274). `Fn`/`Native` is an implementation detail the language does
not have — `(type-of inc)` is `:fn`, `(fn? inc)` is true for both, and the grammar's `fn`
already parsed to both. Only the renderers disagreed, so a warning read `expects keyword |
fn | native` and `to_source` **declined** on `Tag::Native` — leaving the callable type
ADR-272 infers for every callback with no faithful annotation, so the declare-sig surfaces
could not offer the newest and most useful inference. It writes as `(or fn keyword)` now,
and round-trips.

## Adoption, and why it compounds (2026-08-28)

`std/` carries **358** `(sig …)` declarations, up from ~34. That took three rounds, and
the rounds are the interesting part: ADR-277 made a file-local declaration constrain its
callers *in the same file*, so each round sharpened the domains the next one read.
`bytes/int` was `(any (or map number) (or map number) -> any)` — noise ADR-276 rejects —
before its neighbour `bytes/at` was declared; afterwards it was `(bytes int … -> any)`.
Round 2 found 45 newly-adoptable signatures, round 3 found 20, and the remaining ~1550 are
the tiers ADR-276 rejects on purpose (return-only, module-private, and the arithmetic-domain
noise).

Adoption paid for itself in defects, not just documentation. It surfaced ADR-275 (**every**
unconditionally-raising function with a signature was told its body contradicted its
declared return, because `never` is disjoint from itself), an inconsistency where `odd?`
demanded `number` while `even?` demanded `int` for the same argument, and ADR-277 itself.

One rule turned out not to be decidable in advance: a function whose job is to *validate*
its own argument must not declare that argument's type, or its own guard becomes provably
dead code. Guessing that from the source is guesswork the checker has already done, so it
is a feedback step — declare, run `nest check`, drop any declaration that now reports an
`unreachable clause`. Rounds 1 and 2 both re-added the same two functions because the rule
lived only in a human's memory; round 3 dropped them automatically.

The declarations are visible where they are read: `nest doc` renders the type under each
heading now, for a declared signature and a curated primitive alike.

## The backlog is empty (2026-08-28)

Both remaining lattice items shipped, and they turned out to share one foundation —
`P ∖ N = ∅` exactly when `P ⊆ ⋃N`, so the emptiness decision and the subtyping decision are
the same code and cannot disagree.

**A term subtracts** (ADR-288). A tag could be complemented exactly, and since ADR-268 a
literal set, but not a structure: `¬(vector int)` widened to `any`, `(vector int) ∩ ¬(vector
int)` came out `(vector int)` rather than `never`, and `¬¬` destroyed the type instead of
restoring it. A term now denotes `P ∖ ⋃N`, all three are exact, and `(not (vector int))`
finally *checks* instead of merely parsing. The property laws caught four defects on the way
— subtraction-blind absorption, short-circuiting disjointness rules, dropped negative
candidates, and an order-sensitive subtraction list.

**A product can be covered by several alternatives together** (ADR-289). `(tuple (or int
string))` is contained in `(tuple int) | (tuple string)` — a 1-tuple holds one value, which
lands in one or the other — and the per-tag rule from ADR-267 answered false, a false
positive. Decided by the set-theoretic product rule now, with both neighbours pinned as
tests: componentwise coverage is not product coverage, and an arbitrary-length vector escapes
both alternatives. Fixed arity is what makes products different.

Two items the roadmap still listed as deferred were found already **done**, closed by this
session's overload inference rather than by anything aimed at them: per-arm parameter
checking of a multi-arity callee, and exhaustiveness from an *inferred* scrutinee. Both were
verified by probe before the roadmap was updated.

### What's left

- **`sig` adoption itself** — mechanical rather than archaeological now (ADR-276 records the
  criteria, and the arithmetic-domain tier was probed rather than guessed in ADR-284's
  batch), but still a judgement call per declaration.
- **Arrow decomposition** — the one thing deliberately left. `¬(int -> string)` is
  *represented* exactly, but deciding coverage *between* arrow types still falls back to the
  single-candidate rule. Contravariance needs its own decomposition rule, and nothing has
  asked for it: the miss is incompleteness in the safe direction.
