# Type system — status & what's left

**Revised 2026-09-12** (the review at the end of this document — where the `any` tail
actually is, measured); previous revisions 2026-08-27 (the audit's ranked items,
ADR-259..263) and 2026-07-30. Where the type system actually
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
| **The annotation surface** — `sig`, and the checker's reach over program text | `sig` **fails closed** (ADR-259) and the *definition* owns the arity. The walk's totality is now gated (ADR-260), and that gate immediately found the next instance of the KI-67/KI-70 class (quasiquote escapes). A shape has a name: `deftype` (ADR-327) declares a structural alias a `sig` can spell, resolved through the file's own namespace, then its `(:use …)`/`(:alias …)` imports, then the one loaded declarer — and a diagnostic shows the alias, not its expansion. **Strict mode** (ADR-298) reads a positively-known bound by inclusion, with consistent subtyping for nested unknowns (ADR-326); `std/` is at zero in both modes, gated in CI, and bedit holds itself at zero with its own hard gate. |

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

;; a lambda LITERAL as a callback (KI-85): typed under the arrow's own domain
(sig g ((int -> int) -> int)) (g (fn (x) (str x)))  ; result string, used as int
;; …and the wider-result case stays silent: (g (fn (x) (+ x 1))) is int under x : int
;; precision without loss of soundness (2026-08-29): exact where provable, deferred otherwise
(+ 1/2 1/2) (* 2 1/2)       ; int | ratio — ratios close over + - * and /, like ints,
                            ;   and either shape can cancel a denominator away
(+ 1 2 1/2)                 ; ratio — but `+ - inc dec` over ints and exactly ONE ratio
                            ;   cannot: no ratio is integral (denominator 1 demotes to Int
                            ;   on construction), and a whole-number shift keeps q
(reduce + 0 ints)           ; int — a numeric operator folds inside its closure (induction)
(map inc ints)              ; nil | list<int> — the operator's closure, not its widened sig
(cons 1 '())                ; list<1> — a nil tail has no elements
'(1 2) (vec …) (into …) (conj …) (merge …) (apply …) (try …) ((fn …) x) (range n)
                            ; each typed by what it provably is; all were `any`/nil/bare tags
;; a constructed record knows its fields (2026-08-29): `defrecord` declares `?field` vars
(defrecord pt (x y))  (:x (pt 1 2))      ; 1 — was `any`
;; every warning has a position: a lint over the expanded tree (match exhaustiveness,
;; an unreachable clause, an argument inside a destructuring let) is placed at its
;; enclosing top-level form when the expansion left it none
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

All four items that stood here on 2026-08-28 now behave — verified by probe, not by reading
the code (`(get r :ok)` over a tagged union, `(not (tuple int))` against `[1]`, a callback
whose result is wrong, and a product covered jointly by two alternatives). What is left in
this position is not a missing rule but a deliberate **reporting policy**:

```lisp
;; an argument the checker cannot prove is right, but cannot rule out either,
;; is NOT reported when its type is dynamic (ADR-110's gradual relation)
(sig takes-int (int -> any))
(sig maybe (int -> (or int bool)))
(takes-int (maybe 1))          ; silent — a call result is dynamic, so `∩ ≠ ⊥` decides
(defn outer (x) (takes-int x)) ; WARNS when x is a sig-typed param, i.e. precise, so `⊆` decides
```

A **precise** argument (a literal, a `sig`-typed parameter, integer-closed arithmetic) is
checked with `⊆`, so a merely-wider misuse is caught. A **dynamic** one (a call result, a
redefinable global) is checked with `∩ ≠ ⊥`, so only an argument that *cannot possibly* fit is
reported. That asymmetry is the reload-safety guarantee, not an oversight: warning on
merely-not-guaranteed would fire on every `(or T nil)` flowing into a `T` parameter, and a
`def` must always be able to win. See `check/walk.rs` and docs/type-gating.md "B1".

## What's left

Eight of the nine items the audit ranked shipped the same day (ADR-259..263); what follows is
what they left behind, plus the items that were deferred on ADR-011 grounds and still are.

**Re-probed 2026-08-29.** Items 1-4 — the whole *lattice* backlog — are now shipped, and each
was verified by running it rather than by reading the code. What remains is deliberately not
lattice work: two small wiring gaps (6, 8), two large design items that need bidirectional
inference or a compiler channel (5, 7), and adoption (9), which is ongoing by nature.

| # | Item | Why it is left | Cost |
|---|---|---|---|
| ~~1~~ | ~~A field lookup on a tagged union~~ | **Shipped** (ADR-264): records are closed by default, with `&open` as the marked case and openness modelled as the type of the undeclared keys, so `(get r :ok)` over `{ok: int} \| {error: string}` resolves to `int \| nil` and the two arms are provably disjoint | ✅ |
| ~~2~~ | ~~The complement of a refined term~~ | **Shipped** (ADR-288): a term carries `neg`, a list of subtracted types, and emptiness is decided by the identity `P ∖ N = ∅ ⟺ P ⊆ ⋃N` — so one routine serves both emptiness and subtyping. `(not (tuple int))` is exact | ✅ |
| ~~3~~ | ~~A callback's result is never checked~~ | **Shipped** — a parameter in call-head position is intersected with the callable type (ADR-272), and a callback whose result is wrong now reports at the call site | ✅ |
| ~~4~~ | ~~Subtyping across terms is incomplete~~ | **Shipped** (ADR-289): the set-theoretic product rule over subsets of the alternatives, so `(tuple int\|string, int)` is proven under `(tuple int int) \| (tuple string int)`. Extended to arrows by ADR-292, where an intersection satisfies a requirement no single arm does — checked against a brute-force model of what an arrow denotes, 0 unsound and 0 missed in 2.5M pairs | ✅ |
| ~~5~~ | ~~Return-type dispatch~~ — selecting an impl by expected return | **Declined, not deferred** (ADR-361, 2026-09-17): it can only be a checker-driven rewrite, which the advisory rule (ADR-123/124) forbids; generic code names its target as a value (a seed, an exemplar, a receiver). A multimethod keyed on a type designator is the door left open | 🚫 |
| ~~6~~ | ~~Qualified cross-module ability type names~~ | **Shipped 2026-08-29** — and the headline was already done: in a PROJECT check both `Shape` and `shapes/Shape` resolve, because `ability_type` reads the last `/` segment (the registry is keyed by bare CamelCase name, ADR-255). What was broken was the loose single-file fallback, where neither can resolve and the checker falls back to "capitalised means an ability I cannot see" — that test read the WHOLE spelling, so `shapes/Shape` reported `unknown type` while bare `Shape` was accepted. Naming the module an ability comes from must not be what manufactures a diagnostic | ✅ |
| 7 | **Tier-2 monomorphization** — devirtualizing an *inferred-variable* op call | Still deferred, and now for a better-founded reason. Turning `BROOD_MONO` on for the first time (2026-08-29 — **nothing in the repo had ever set it**) found Tier 1 miscompiling: it baked the resolved impl *value*, and a body compiles before it runs, so a module registering an impl and using it in the same body called the wrong one. Fixed by proving the identity and leaving resolution behind the epoch-guarded cache (ADR-294), and gated by a differential. Tier 2 multiplies that surface across every call site the checker can type and still needs the checker→compiler channel — but it now has a sound base and a gate that catches a miscompile the first time the flag goes on | Large |
| ~~8~~ | ~~Runtime contracts for ability ops~~ | **Shipped 2026-08-29** (ADR-293): `impl` wraps a method whose op declares `:-> RET`, decided at expansion time so an unset flag emits nothing. Building it revealed `BROOD_CONTRACTS=1` had rotted into *unusable* — three cold-boot-cache-only defects, none of which any gate could see, because the mode had no end-to-end test at all (KI-81) | ✅ |
| 9 | **`sig` adoption across std** — **787** declarations over **3357** `defn`s on 2026-09-12 (407/2942 on 2026-08-29; 34/2828 on 2026-08-28), plus 12 `deftype`s | Every one now buys more than it did: a declared sig is what the reversed-args gate (KI-71) reads, and inference is checked against it (ADR-259). The strict sweeps of 2026-08-30 and 2026-09-07 were mostly this | Ongoing |

**On adopting in bulk (2026-08-29).** `nest check --suggest-sigs` prints what the checker
would infer, and it is advice rather than a patch for a good reason: an inferred domain
over-approximates, so pasting it verbatim enshrines nonsense as documentation —
`(sig url/url-unreserved? ((or map number) -> bool))` for a character predicate is the
suggester working correctly and the declaration being wrong. The batch that landed
(`encoding`, `stats`, `multimap`, `math`, +38) was written by reading each body. The payoff is
concrete rather than decorative: `(stats/percentile 50 [1 2 3])` now reports on **both**
argument positions, where before it was silent.

**Precision residues, still sound to leave.** The *merely-wider* residue — a body typed
exactly `number` declared `int` (e.g. `(/ x 2)`) — needs occurrence/range analysis to pin and
would false-positive if flagged (ADR-011). Element-typed `seqable` is still unrefined; a
genuine element-typed seqable needs extending the `elem` refinement beyond `Pair|Vector`.

---

## Inventory — what exists

### The lattice (`types.rs`)
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
| Lattice (`Ty`, ops, named unions), `GradualTy`, `Sig` | `crates/lisp/src/types.rs`, `types/sig.rs` |
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

**`reflect/source-signatures`** (2026-08-29) is the same question for source *text* — an
editor buffer mid-edit, or the one form a live evaluator just ran, neither of which is a
file. Same maps, same checker pass, one shared renderer; `()` rather than an error on
unparsable input, matching `check-string-structured`. It exists because `expr-type` cannot
answer for a definition at all: a `(defn …)` form evaluates to its own *name*, so the type
of its value is the type of a symbol and says nothing about the function.

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
`types.rs` changed.

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
- ~~**Arrow decomposition**~~ — shipped 2026-08-29 (ADR-292), and it was not the safe
  incompleteness it had been recorded as. `(and (int -> int) (bool -> bool))` really is a
  `(int|bool -> int|bool)`, the single-candidate rule answered *false*, and multi-arity
  functions are exactly the intersections of arrows this language is built from. The
  set-theoretic rule now decides it, reusing ADR-289's product covering for the domain half
  rather than growing a second covering algorithm. Checked against a brute-force model of
  what an arrow denotes, not against more property laws — a more permissive relation is the
  one direction that can be unsound, and laws that check the relation against itself cannot
  see it: **0 unsound and 0 missed containments across 2 547 216 pairs.**

## Six precision items, and the position a macro threw away (2026-08-29)

Six items from the soundness review, each a tightening in the safe direction; all six are
pinned by tests in `types::check::tests`.

1. **A warning inside a `match` clause points at the clause** (ADR-297's promise, kept
   for the one macro that broke it). `%match-splice-fail` inlined the fail continuation by
   rebuilding the *whole* compiled tree with `cons` — every clause body included — so the
   reader's pairs, and their positions, never reached the expansion; the stamp could only
   inherit the `match`'s line. It now rebuilds only the spine above a splice point and
   returns an untouched subtree as the very pair it came in as. A rewrite should rebuild
   what it changes and nothing else — that is what keeps positions (and sharing) alive
   through expansion.
2. **`nest check --strict`** (ADR-298) — a dynamic value with a precise bound is checked by
   inclusion. Off by default; `BROOD_CHECK_STRICT=1` also turns it on.
3. **`list<A> ∩ list<B>` is empty for disjoint `A`, `B`.** A `list<T>` is the `pair` tag
   alone (the empty list is `nil`), so every value has a first element and no first element
   is both an `int` and a `string`: `list<never>` is uninhabited (`term_is_never`), and
   `is_disjoint_term` answers the same — the argument check (`!is_disjoint`) and the lattice
   used to contradict each other here, which is why `(want-strs (list [:a 1]))` was silent.
   `vector<never>` and `set<never>` stay inhabited (the empty vector, the empty set).
4. **`merge` keeps the shapes.** `(merge {:a 1} {:b 2})` is `{a: 1, b: 2}`, not `map`; the
   later map's fields win, exactly as the value does.
5. **A type variable inside `or`/`and`.** `(or ?A nil)` binds `?A` to the argument *minus*
   the concrete alternatives, so `(or-default n 1)` under `((or ?A nil) ?A -> ?A)` is the
   `int` `n` was, with `nil` carved off; `(and …)` unifies each part.
6. **Sets carry an element type** — `set<E>`, with the same `elem` refinement lists and
   vectors use (`SEQ_BITS` gained `Set`): the literal `#{1 2}` is `set<1 | 2>`, `#{}` is
   `set<never>`, `conj`/`into` onto a set keep it, and `(set T)` is accepted in a `sig`,
   variables included.

One diagnosis on the way is worth keeping: `(+ 1 1/2)` typing as `ratio` looked like a
regression of the ring rule and was the *additive int-plus-ratio* rule doing its job — a
reduced ratio shifted by whole numbers keeps its denominator. The two tests that expected
`int | ratio` predated the rule.

## `(or map number)`, retired (2026-08-29)

The one output the review kept tripping over: `(defn foo (x) (+ 1 x 1/2))` hovered as
`((or map number) -> (or map number))`. It was the `Num`-record widening — `+` accepts a
record with a `num/add` method, and a record is a map — stated as the widest thing that
was true. ADR-299 states it as the exact thing that is true: an operator's domain is
`number` plus the cover of the multimethods it routes to, read off the registry at the root
of each check. No record loaded → `number`; `usd` loaded → `number | t/usd`, rendered by
name (a record inside a wider term now prints its identity, not the tag `map`). The rule is
general — `MultiInfo::domain_ty` is any multimethod's parameter type — the operators are
just its first consumer.

## What was left, measured (2026-08-29)

`nest check --suggest-sigs` over std writes 581 signatures. Tallying what a reader would not
paste gave the list, in order of yield:

- **73× `(or rope seqable string table)`** — `count`'s domain, a set the checker had under
  the name `countable` in Rust and could not spell. Now `countable`.
- **63× the comparison cover**, spelled out — see ADR-299's addendum: `ordered`, `numeric`.
- **16× `(record :__id__ :datetime/datetime :day number …)`** in return positions: a
  nominal shape is now spelled by its name in a `sig`, the inferred field refinements
  dropped (the name denotes the open `:__id__` shape, a supertype — sound to declare, and
  what a reader writes). `Display` keeps the refinements for diagnostics.
- **`math/max`/`math/min` returning `(or map number)`** — routed through the operator
  domain like `<`.

After: 0 raw record shapes, 0 six-way countable unions; `ordered` 61×, `countable` 73×,
`(or map …)` 13× (all `send`'s `map | pid` target, which is what it is).

**The first `--strict` run over std** (~230 warnings) sorted into families. Two were
defects in strict, fixed: a bound known only by exclusion — `(not nil)` from a `when`, the
truthy half of `(or x default)` — kept being read by inclusion (now `is_known_only_by_
exclusion` keeps the overlap rule), and that truthy half rendered as a 21-tag list (now
`(not (nil | false))`). The rest are what strict is for: `nil | string` from `nth`/`first`
handed to a string function, `number` where `int` is declared, a declared `int` return over
a body that yields `number`. A `sig` on the function is the answer to each.

**What is genuinely left: the `any` tail.** 404 of 581 suggestions contain `any` — a
parameter only passed through, or whose only demand comes from inside a conditional branch
(`(>= i n)` runs on every path of `ansi-csi-end`; `(nth v i)` and `(+ i 1)` do not, so a
sound meet cannot use them). Narrowing those needs demand flow across functions and
polymorphic (`?A`) suggestions — the inference frontier, and where an unsound rule would
hide, so it wants the brute-force-model verification ADR-292 used, not rules by hand.

## The `any` tail, first cut (2026-08-29)

`(defn foo (x y & more) (+ (fold + x more) y))` suggested `(any any & any -> number)`. Two
causes, both in demand inference: a `& rest` clause returned no demands at all ("no
positional signature" — but the fixed parameters bind positionally regardless, and the rest
binder's demand on its list is a per-argument demand on the list's element type,
`Sig::rest`); and `fold`/`reduce` typed their *result* through the callback but never handed
the callback's demands to the init and the collection. With both, `(number number & number
-> number)`. A third, on the return side: the tail's type was inferred with every parameter
bound to `any`; it is now inferred under each parameter's demand — what every call that
reaches the tail satisfied — so `(fold + x xs)` is numeric, not `any`.

## The length fact, carried (2026-08-29)

`(append '(1 2 "foo") '(1 "bar"))` hovered as `nil | list<1 | 2 | string>`. The `nil` was
the empty-input case, stated for every input; but both inputs here are `list<…>`, which
since the list-disjointness change is the NON-empty list — so the result cannot be `nil`.
That one fact (`provably_non_empty`: `⊆ pair`) now flows through every combinator that
preserves length (`map`, `sort`, `reverse`, `distinct`, `append` with one non-empty
argument, `into` a list, a literal `range`) and through `first`/`last`, and is withheld from
every one that can empty a sequence. Sound in the only direction that matters: `nil` is
dropped exactly where no input can produce it.

The demand walk also consults a module's own inferred signatures now (`ctx.inferred_fn_sig`
— a table lookup on the fixed-point pass's result, so no recursion): all-`any` parameter
lists over std went 203 → 182, `-> any` returns 117 → 108.

## Strict to zero over std (2026-08-30)

The plan (recorded in the devlog): strict mode + `sig` adoption to zero on std FIRST, because
it measures how much of the `any` tail annotations remove before inference is built for it.
The first `--strict` run over std gave **336** warnings; the tree is now at **0**, and
`nest check --strict std/**/*.blsp` is a CI gate beside the non-strict one.

**What the warnings were.** Three families, only one of which was "add a sig":

- `expects int, got ordered (n)` / `expects int, got number` (~200): an unannotated
  parameter whose inferred DOMAIN (`ordered` from a `>=`, or nothing at all) strict reads
  as its type. A `sig` on the enclosing function is the answer — ~350 were declared,
  written by reading the bodies (a wrong sig is checked against the body and at every call
  site, so it cannot be pasted from `--suggest-sigs`).
- `got nil | string` / `nil | number` (~60): `nth`/`first`/`string/->number` results handed
  on unguarded. These are what strict is FOR; the fix is in the code — `(or (nth parts 1)
  "")`, an unparsable component reading as an out-of-range `-1`, a `match` on the item
  instead of `(first item)` after a `(= item :done)` test. Every default is semantically
  inert; one was a real bug (`package/registry-install` handed a keyword to `path/join`).
- The rest were CHECKER gaps, each fixed generally rather than worked around in std:
  an extremum returns one of its operands; `(get m k default)` / `(nth xs i default)`
  read their default as the absence case; `(or x default)` is short-circuit exact and an
  inferred return sees its branches narrowed (`guards::branch_scopes`, shared by all three
  `if` readers); a record NAME in a sig carries its declared field types (open, ADR-264);
  a defaulted `&optional (n 1)` is `T ∪ typeof(default)`, not `T | nil`; a destructuring
  `let` types its binders (`%vector-ref` is exact behind the length check); `(= a b)` is
  the `%eq` guard; a branch whose test contradicts what is known of a local is dead and
  neither checked nor typed (`Ctx::is_dead`); `any ∖ (a finite literal set)` and `any ∖
  vector` are known only by exclusion; `filter` returns a list; a `fold`/`reduce`
  accumulator is seeded from `init` and taken one step to a fixpoint, and a `fn` literal
  in that position is walked with the same seed; the prelude's own `sig`s ride into the
  prelude region (they were dropped at the freeze); a `sig` inside a `check-allow` block is
  registered; the in-file record field-type table is built before the ability facts.

**What strict does not do, and why that is right.** A record name is an OPEN shape, so a
key it does not declare reads as unknown — `(get date :hour 0)` on a `date` is not `int`;
the honest form is a declared accessor. `datetime?`-style user predicates did not narrow
at the time (the checker knew the built-in `tested_by` predicates only) — the **type-guard
signature** `(sig datetime? (any -> (is datetime)))` shipped the same day as this section's
sweep (ADR-301, 2026-08-30) and is the general answer. `tests/` was not held to strict at
the time (a test hands a sig the literals it must reject); it joined the gate on
2026-09-17 — the deliberate mismatch says so with `(check-allow :type-mismatch …)`.

**Measured on the way.** The demand walk consulting a loaded module's inferred sig costs
nothing (the zero-warning gate over 342 files: 5.0 s with it and without). `nest check`
resolves a `:use`d std module from the BINARY's baked-in std, so a cross-module sig change
is only visible after a rebuild — a batch that edits one module and checks another must
rebuild in between, or it verifies the old sig.

## Narrow first: the division residue, and the two kinds that had no elements (2026-09-02)

Two items the "what's left" list had recorded as small and left alone. Both are
tightenings in the safe direction, and both were sized by *running* the corpus rather
than by reading the rule.

**The decidable half of `(/ int int)`.** The roadmap's order was narrow first, flag
second: `int | ratio` is the honest answer for `(/ x 2)`, but the entry listed four
shapes that are not undecidable at all, and until they stop landing in that union no
declared-`int` residue can ever be flagged. Three are decided at the **type** level off
the int-literal refinement (ADR-117), which means `numeric_result` owns them and a
callback (`(map xs (fn (n) (/ n 1)))`) and a fold get them for free:

- a literal **±1 divisor** keeps the numerator's kind — `(/ x 1)` is `x`, `(/ x -1)` is
  `-x`, so `int` stays `int` and `ratio` stays `ratio`. The numerator's own literal set
  is deliberately dropped rather than carried through: `(/ 6 -1)` is `-6`.
- every operand a **known int literal** folds exactly: `(/ 6 3)` is `int`, `(/ 5 2)` is
  `ratio`, `(/ 6 4 2)` is `ratio`, and unary `/` — the reciprocal — makes `(/ 2)` a
  `ratio`. A literal *set* landing on both kinds stops the fold and keeps `int | ratio`,
  which is the answer already given.
- a **zero divisor declines**. `(/ 6 0)` raises E0040, and typing an expression that
  cannot produce a value would be stating the arithmetic instead of what the language
  does.

The fourth and fifth shapes — `(/ (* 2 x) 2)` and `(/ x x)` — are left. Both need
form-level syntactic analysis, and neither is a shape anybody writes; they were artifacts
of probing the rule, not findings from a corpus.

The payoff is in both directions, which is what makes narrowing worth more than the
deferral it unblocks. Two correct programs stop carrying an unprovable union — and
`(defn c (x) (/ 5 2))` declared `(int -> int)` is now a **named finding**, because Brood's
`/` is exact and `5/2` is a ratio. That is the mistake a newcomer brings from a language
where `/` on ints truncates, and nothing else in the tree could name it.

**`bytes` and a map's entries.** The two `seqable` members with no element type. Neither
needs a refinement, because the kind decides the answer: a `bytes` is a sequence of
octets, and a map walks as its `[key value]` entries — a two-element **vector**, checked
against the runtime rather than assumed. Both are derived inside `Ty::elem_ty`, the choke
point every consumer already goes through, so `first`/`nth`/`map`/`filter`/`fold` picked
them up at once: `(first (string/->bytes s))` is `nil | int`, and
`(map m (fn (kv) (first kv)))` over a closed shape is `list<:a | :b>`.

Two gates came out of building it, and each was found by running something rather than by
reasoning about it:

- **A derivation may only speak when the term admits ONE collection.** A refinement the
  type *carries* is a different matter — it was put on the sequence members deliberately,
  and "a seqable of numbers" is exactly that shape, which is how a `& rest` binder's
  demand is spelled. But a *derived* answer speaks for the whole term, so `bytes | vector`
  has unknown elements. The unit suite caught the first cut, which tightened the carried
  case too and broke that demand.
- **An open shape yields nothing, and that gate is what keeps NOMINAL records out.** The
  first cut answered `(tuple any, any)` for a map it knew nothing about — reasoning that an
  entry is at least a pair of unknowns. It is not: a record is modelled open, a record may
  implement Seqable, and then it walks as whatever that impl yields. `tests/queue_test.blsp`
  maps over a queue, and the **checker gate** flagged its callback — a false positive that
  no unit test would have produced, since the shape only exists in a file that defines an
  ability impl.

**And the length fact reaches two more shapes.** `provably_non_empty` was `⊆ pair`. A tuple
states its arity and a closed record states its keys, so either carries the same "has a
first element" fact a `list<T>` does: `(first [1 2])` is `1`, not `1 | nil`, and
`(map [1 2] inc)` is `list<int>`. An optional field cannot carry it — it may be absent,
which is the empty case.

## The converse lint, which needed no effect system (2026-09-02)

`deferred.md` had the "this can fail and nothing guards it" lint blocked behind an inferred
`nothrow`-shaped bit, on the reasoning that the checker cannot know which functions yield a
failure. **It already knows.** `failure` is a tag, so it rides the ordinary union: the
producers declare it, and an unannotated wrapper infers it — `nest check --suggest-sigs`
writes `(string -> (or failure number))` for `(defn parse (s) (string/->number s))` with no
annotation anywhere. The gap was a *reporting* rule, and it is one line of lattice: a
**failure is never a valid materialisation** of a domain that excludes one, so it is read
by inclusion in both modes where every other arm of a union keeps the overlap reading
(ADR-316 carries the argument for why `failure` and not `nil`).

Measured before shipping, because the cost is the whole question: the rule fires on exactly
the failure sites and nothing else — **0** across `std/`, **6** across `tests/` +
`examples/`, and **8** in bedit. The six are written-out literals that cannot fail whose
*type* still carries the arm, and carry `check-allow`. The eight are all real bugs, and all
one shape — a `nil?`/truthiness guard written before ADR-310 made a failure **truthy**, so
a failure walks straight through it into arithmetic that raises. That is the migration
hazard ADR-310 predicted and could not name; this names it.

Found while measuring, and worth more than the lint on its own: **the incremental check
cache was not keyed on the checking mode.** A verdict depends on the mode that produced it,
and the cache stored mtime, dependency fingerprint and require-closure — none of which move
when `--strict` is added. So a plain `nest check` cached its verdicts and the next
`nest check --strict` over the same files reused them, reporting what the plain run had
found. CI runs the two gates back to back over `std/**`; the strict gate would have gone
quiet the moment the plain one warmed the cache, and a passing run is all anyone would have
seen. (`std/` really is strict-clean — checked with `BROOD_NO_CHECK_CACHE=1` — so nothing
was hidden in fact, only in principle.) The manifest name now carries the mode, so each
keeps its own warm cache, the way `"checks"` and `"checks-run"` already do; the new
`reflect/strict-checking?` is how Brood asks. `crates/nest/tests/check_cache_mode.rs` runs
plain-then-strict-then-plain over one unchanged file and is sabotage-verified.

**A review pass found one of these unsound, the same day (2026-09-02).** The length fact
read "has a first element" off a record's SHAPE, and a shape survives a union with `nil`:
`(first (if p {:a 1} nil))` answered `(tuple :a, 1)` and dropped the `nil` every caller has
to handle. `provably_non_empty` now requires the type to be ONLY that collection, which is
what `is_subtype` states and a tag test would not. Two details worth keeping:

- The **record** gate is the one that fires. The vector one cannot — a union with `nil`
  widens a tuple shape away (`nil | (tuple 1)` is `nil | vector<1>`), so `tuple_elems()`
  already declines. Sabotage said so: removing the vector gate reddened nothing. It is kept
  as a defence, with the union-widening property pinned by its own lattice test, so the day
  that widening improves the gate is already in place and the pin says why. A guard that
  cannot fail reads as coverage; a guard that cannot fail *and says so* does not.
- The rule was checked from the other side too — every position that legitimately CARRIES a
  failure stays silent: `=`, storage in a collection, a vector literal, `str`, returning it,
  and above all `ok->` and `with`. A lint that fired on the two mechanisms ADR-315 provides
  as the answer to it would be fighting the language.

## Declarations that said nothing, and the guard idiom that forced them (2026-09-03)

A hover reported `(string -> any)` for a body that plainly yields `0 | bool`. The checker
was not being imprecise: `std/regex.blsp` **declared** `(sig match? (any string -> any))`,
and a declared sig is authoritative (ADR-259). The library was telling the checker to forget
what it knew. `handoff.md` records this trap for *curated* sigs and
`no_declared_std_sig_widens_its_curated_signature` gates that half; a Brood function with no
curated counterpart had nothing watching it.

**Measured by asking inference.** Strip each file's `(sig …)` lines, run `--suggest-sigs`,
compare: 119 declared `-> any` returns, and inference proves something narrower for **31**.
Adopting an inferred return is sound — it is an upper bound on what the body yields — so 28
were taken verbatim. Three were skipped because the inferred type is an artifact rather than
a contract: a datetime cover from ADR-299's operator domain, a raw record shape (the same
thing `--suggest-sigs` was taught not to print), and an `ordered` from an extremum. The five
`?`-predicates were fixed by *reading the bodies*, not by adopting: four are `bool`, and
`format/multi-arity-defn?` is `(or nil bool)`, because `(and lead …)` yields `lead` when it
is falsy — a Brood predicate is not automatically a `bool`.

**Then the part worth keeping.** Narrowing `project/find-root` to its honest `(or nil
string)` took the strict gate from 0 to 17, and 16 were one shape:

```brood
(let (root (project/find-root (file/cwd)))
  (when (nil? root) (error "not in a Brood project …"))
  (project/setup root)      ; ← "expects string, got nil | string"
  …)
```

Every one of those sites is **correct**. Brood has no early return — no `return`, no
`guard` — so "refuse and stop" is `(when bad (error …))`, and the checker did not know that
reaching the next form proves the guard false. Which reframes the whole exercise: the wide
`-> any` was not laziness, it was the only way to silence a rule the checker was missing.
Fixing the signatures without fixing that would have pushed sixteen false positives into
`std/`.

`walk::diverging_guard_scope` — a body form `(if COND THEN [ELSE])` with one arm typed
`never` narrows every following form to that arm's complement. Both directions, since
`(when t b)` lowers to `(if t (do b) nil)` and `(unless t b)` to `(if t nil (do b))`. Sound
because it is the ordinary else-scope of the condition, applied on the path where the other
arm provably did not run.

**It had to be added twice**, which is the same walk/inference split that let ADR-316
false-positive two days running: `check_let` for the walk, `infer::sequence_scope` for
inference. Inference types a body as its LAST form and never looked at the forms before it —
right for values, wrong for scope. With only the walk, the guarded function checked clean
and its inferred *return* still carried the `nil`, so every caller was reported instead of
it. Twice now the lesson has been the same: **a narrowing that only the walk knows is a
narrowing the callers do not get.**

## Nine days on: the review, and where the `any` tail actually is (2026-09-12)

The status above was last revised on 2026-09-03; this section is what a review of the
type system found on 2026-09-12, measured against the tree rather than read off the
documents — which had drifted in three places (type guards listed as "the next item"
thirteen days after they shipped; the `sig` count at 407 against 787; "the backlog is
empty" written before ADR-326/327 and the week's element rules).

**What moved in the nine days.** Strict inclusion became *consistent subtyping*
(ADR-326): a nested unknown — a record field, a vector's elements, a map's shape — is the
gradual `?` wherever it sits, so a strict warning points at the parameter whose type is
missing and never at a field that inherited its unknown-ness. `deftype` (ADR-327) gave a
structural shape a name a `sig` can spell, and a diagnostic prints the alias rather than the
forty-field record it stands for. The element rules landed one at a time from bedit's
hover: `map`/`filter`/`mapcat`/`seq/keep`/`seq/remove` answer a `list` exactly, `seq/find`
the element or nil, `seq/remove` with a type predicate keeps what it rejects, a field read
over `nil | record` is `nil | field`, `update`/`assoc-in`/`update-in` keep a record shape,
a fold over a provably non-empty sequence is its step result, an impl's `self` is the
record it dispatches on, a multimethod's params are seeded from its dispatch key. Two
strict sweeps took `tests/` from 269 findings to the nine KI-116 records as the checker
being right, and bedit from 461 to **zero** — where it now holds itself with a hard gate
of its own (`tests/strict_ratchet_test.blsp`).

**What the review found, by probe.**

- The **std strict gate was red** on every CI run of the day — three findings in the
  morning's editor commits — and bedit's smoke was red on a stale `BEDIT_REF`, eight
  commits behind bedit's own strict-zero sweep. Landed separately.
- A **strict false positive** in the fold accumulator, bisected to one shape:
  `(fold xs [0 '()] (fn (st x) (let ([j acc] st) [(inc j) (cons j acc)])))` infers
  `(or (tuple 0 nil) (tuple number pair))` — the `j` slot widens to `number` when the
  OTHER slot changes, and stays `int` when it does not (`(tuple int nil)`); a scalar
  accumulator is right. Two causes, both fixed the same day: a position read over a UNION
  of tuple terms fell to the whole-element union (`tuple_elems` answers only for one term;
  `Ty::tuple_elem_at` is exact over the union), and the fold fixpoint took exactly one step
  where a tuple whose slots feed each other stabilises on the second — it iterates, bounded.
  `[0 '()]` under `[(inc j) (cons j acc)]` is `(tuple int list<int>)`.
- **`BROOD_CHECK_STRICT=1` reached `nest check` only.** The flag catalogue and ADR-298
  named it as the strict switch; `brood --check`, the REPL, the LSP and
  `check-string-here` ran plain with it set. The env read is the kernel flag's own
  default now (`types::strict_checking`), so every entry point reads it alike; guarded by
  `crates/cli/tests/strict_env_flag.rs` on a strict-ONLY finding.
- **KI-129** — a script outside `src/` loaded a project module twice, rooted and bare, so
  every `deftype` in it was ambiguous. The cause was one level below the entry's
  diagnosis: `spawn_root_program`, the process `brood FILE` and `nest run FILE` run in,
  built its heap without the package-context inheritance an ordinary `spawn` has carried
  since ADR-070. Fixed and guarded.
- `tests/` strict has drifted 9 → 21 since KI-116 (not gated, by design); four of the
  new ones are one missing `(sig pop-mark (buffer -> buffer))`.

**The `deftype` follow-ups ADR-327 scoped out, done.** An alias resolves through the
file's imports — a bare name to the ONE `(:use …)`d module declaring it, `short/name`
through `(:alias mod :as short)` — between the own-namespace step and the loaded-wide
unique-suffix rule, and two `:use`d declarers still decline (the checker now
`ensure_loaded`s an alias clause's target, as the runtime `require-one`s it).
`nest doc` and the doc site render a module's aliases under a **Types** heading, read
from the new `reflect/type-aliases` (a `deftype` binds no global, so the name walk cannot
see it). A recursive alias is **unrolled one level** before its self-reference reads as
`any` — `(:v (:l t))` over `(deftype tree (or nil (record :v int :l tree :r tree)))` is
`nil | int`, where it was the unknown — with the level past that pinned as `any`. True
recursive types (coinductive subtyping, display, round-trip) stay deferred; this is the
decidable part.

**The `any` tail, classified.** `--suggest-sigs` over std writes 1930 signatures, 1307
containing `any`, **624** with an all-`any` parameter list. Two general demand rules
came out of reading a sample of forty, each sound by the same argument as a call's
arguments; one was kept, and both decisions are pinned in
`types::check::tests::effective_signatures`:

- **A literal carries its elements' demands.** `[(- a b)]` and `{:k (- a b)}` left `a`
  and `b` at `any` while `(list (- a b))` typed them — the demand walk had the hole KI-70
  closed for the checking walk. Vector, map (keys too) and set literals now fold their
  elements' domains. 636 → 624.
- **A keyword in call-head position demands a keyed argument — built, measured, and
  dropped.** `(:end a)` raises on anything but a map, a set or nil (verified, not assumed),
  so `nil | map | set` is a sound domain for `a`. It moved one signature over std (std
  reaches for `get`) and manufactured **40 strict findings in bedit**, one per unsigged
  function that reads a model's field and hands the model on to a declared `model`
  parameter — strict reads a positively-known bound by inclusion, and this rule turns the
  commonest idiom in the language into a positive claim. That is the "declare the whole
  program at once" trap ADR-326 removed for map *shapes*, re-entered from the side. The
  decision is pinned (`a_keyword_call_is_not_read_as_a_demand`), so the next "obvious"
  demand rule gets measured downstream before it is kept.

And one contract: `index-of` declared `(any any …)` "because it is polymorphic", which
made every parameter handed to it `any` in every module; it is `(or nil string seqable)`,
which is what the body accepts. `seq/enumerate` and `seq/zip` had none; `enumerate`'s
`(tuple int any)` element is what types the index of an indexed fold.

**And the general callback seed**, found landing the strict gate: a lambda handed to a
function whose signature declares an arrow at that position was walked with NO knowledge
of the arrow — only `fold` and the named element combinators seeded their callbacks, so
`(sig rect-fold-lines (… (buffer int int int -> buffer) -> …))` bought its lambdas nothing
and strict reported the arithmetic the arrow declares as `int`. `arrow_callback_seed` is
the general case, bound the way the element seed binds (as inferred, since a declared arrow
may over-approximate). Eleven findings in `std/editor/buffer` closed by one rule.

**A limitation met on the way, left open:** a type variable inside a refinement inside a
union does not bind — `(sig zip ((or nil (list ?A) (vector ?A)) … -> (list (tuple ?A ?B))))`
leaves `?A` unbound against a `nil | list<int>` argument, so `zip` cannot carry its
elements' types today. `(or ?A nil)` binds (the 2026-08-29 rule); the refined case
needs unification through the alternatives.

What remains is **not an inference gap**, and the sample says so with numbers: of the
624, **152 are predicates** (`list?`, `ws?`, `hl-close?`, `tempo?` …) whose domain
genuinely is everything; ~100 render to a string (`str` accepts anything); 25 hand their
argument to a closure a `spawn` may never run (a demand that cannot be credited — sound);
a further band uses the parameter only inside a `cond` test that runs conditionally
(the sound meet cannot use it, ADR-261); and the residue is primitives whose *contracts*
say `any` (`%digest`, `seq`, `%check`) — sig adoption, one primitive at a time, not a
mechanism. The polymorphic `?A` suggestion the earlier revision named would change the
*spelling* of a pass-through function (`(?A -> ?A)`), not narrow anything. So the
"inference frontier" this document has carried since 2026-08-29 is closed as measured:
the checker infers what a body demands; what it cannot infer is what the body does not
demand.

**Still deferred, unchanged**: return-type dispatch (item 5 — bidirectional inference),
Tier-2 monomorphization (item 7 — the checker→compiler channel, on ADR-294's sound base),
true recursive types, contract blame and contracts-by-default (roadmap 10/11, ADR-153),
parametric abilities, view patterns.

## Declare at the leaf, derive from the call (2026-09-13, ADR-340)

A review of this document against the tree found that the two closing verdicts above — "the
backlog is empty" and "the inference frontier is closed as measured" — were written about the
*lattice* and the *demand walk*, and held for those. They did not hold for what a caller gets
back. Probed, not read:

- **`(defn sum-to (i acc) (if (= i 0) acc (sum-to (- i 1) (+ acc i))))` returned `any`** at
  every call — the accumulator loop, the commonest idiom in a language with no loops. The flat
  answer is right (`acc` is whatever the caller seeds); the call-site answer was never computed,
  because `specialized_ret` declined any self-recursive body. It is a joint fixpoint over the
  parameters and the result now: `(sum-to 10 0)` is `int`, a recursive list builder is
  `nil | list<int>`, and the user-defined `my-map` — also self-recursive, so the "polymorphic
  `?A` suggestion would only change the spelling" argument above did not cover it — types its
  callback's result through `cons`.
- **A loaded closure was inferred by a weaker inferencer than its own file.** The roadmap's
  "inferred parameter types flag wrong callers, cross-file" was true only for a body whose
  parameters were passed *directly* to a primitive: a one-call body's nested arguments were
  never walked by the loaded-closure path ("Tier 1", now deleted). `(defn pad-name (name n)
  (str (string/upper name) (+ n 1)))` checked its callers in its own file and from no other
  module.
- **Inference stopped at the first unmaterialised module.** `buffer-current-line` calls
  `text/char->line`, declared `(rope int -> int)` at the leaf; under lazy loading `text` is not
  materialised in a checking process unless something in the checked file names it, and the
  inferencer read `-> any`. The checker now materialises, transitively, every module the loaded
  bodies name — so the leaf declaration is what the derivation reaches, and a `(sig
  buffer-current-line …)` would have been a declaration of something already known.
- **A `let`-bound lambda checked nothing at its calls**; it carries its parameter domains as a
  per-name fact now. **A private constant had no value type** (`(def- col 10)` read as unknown
  through its privacy expansion; a declared `int` sum of two of them was reported).

**Measured after.** `std/` at zero in both modes; `tests/` strict 22 → 20 (the four `pop-mark`
findings above derive now); bedit plain at zero, strict showing ten of the `nil | int`-from-
`first`/`nth` class its own sweep fixes in code. The whole-std strict gate is ~13% slower
(debug), all of it the fixpoint. Three real findings in `std/` fell out — a `string/bytes->`
declared narrower than its primitive, a `(list a b)` that was a tuple, a `cond` that was a
`match` — and one lattice defect: `nil | list<3>` ∪ `list<int>` kept two terms.

**What is still declared that should derive** — the honest residue, and the next mechanism:
`json`'s index-returning helpers (`json-escape` and three siblings) are declared `int` because
the walk checks a body under its parameters' bottom-up *demands* (`(+ i 1)` says `number`) with
no view of what the callers pass. Caller-derived parameter types for module-private functions —
a closed caller set, so the union of the call sites' argument types is a sound binding when
the name never escapes as a value — is what removes those, and it needs its own measurement
(a second walk per file, or a stored-scope collection pass).

**Still deferred, unchanged**: return-type dispatch, Tier-2 monomorphization, true recursive
types, contract blame and contracts-by-default, parametric abilities, view patterns; strict's
arrow inclusion reading an unknown lambda result as `any` rather than `?`. The computed
callee `((cur 1) "x")` is no longer on this list (2026-09-13): an arrow in head position
describes the call as a named function's signature does — the result is the arrow's, the
arity is exact and each argument meets its parameter through the one per-argument rule
(`walk::check_arg_against_param`, extracted so a named and a computed callee cannot
diverge; `check_computed_call`). A record of handlers `{:len (fn (s) …)}` types
`((get h :len) "x")` from the literal's own arrow.

## Caller-derived parameter types (2026-09-13, ADR-341)

The residue ADR-340 left — four `json` index helpers declared `(… -> (tuple int int))` because
the walk read a body under its parameters' bottom-up demands — is gone, and so is `hex-val`'s
and `json-value`'s declaration: **`std/json.blsp` is strict-zero with no signature on its parser
chain.** `int` (and `vector<int>` for the codepoints) flows from `decode` down ten private
levels, because a `defn-`'s callers are all in its file and Pass 2.9 binds its parameters to
the union of what those callers pass — a least fixpoint, jointly with the private returns
(`(+ i 1)` is `int` under the callers where the demand alone said `number`), the sites read in
the scope the walk sees there (a `let` binder, an `(and j …)` alias, an `if`'s narrowing).

Three things the ascent needed and now has: a call with an uninhabited argument types as ⊥
(unknown was absorbing, and a fixpoint seeded at ⊥ could never rise); tuples of one arity
merge by position (exact when one position differs; eight `[<literal> (+ i 1)]` branches used
to collapse to a bare `vector` past the term cap); and a widening operator
(`Ty::widened_below`) for the round when a JSON value's `vector<… | vector<…>>` would otherwise
nest one level deeper forever.

What derivation does NOT reach, by design: a function handed somewhere with no promise of
what it will be called with (`(apply helper xs)`, a value in a map, an argument to an
unknown function), a file with an unexpanded macro call, callers in another file
(`(:use-internals mod)` included) — those read the demand-based loaded inference as before.
A HANDOVER to a combinator is not an escape (third cut, the same day): `(map xs helper)`
calls `helper` with `xs`'s elements, `(fold xs init helper)` with the fold's accumulator and
an element, a callee with a declared or inferred arrow with that arrow's parameters — the
three promises the walk already seeds a `fn` literal's parameters from (`walk::callback_seed`,
now one entry with a caller-supplied "does this argument fit" predicate), and each is a site
of those types (`Site::Handover`). The fold case is a joint fixpoint with the callback's own
return, as the `fn`-literal seed always was. And what it surfaced in
`json` was the `nth`-answers-`nil | int` class once more (`(digit? (nth s i))` under a `(< i
n)` the checker cannot tie to it): `(nth s i -1)` says what the guard says, and a `->number`
after `strict-number?` unwraps the failure it cannot get.

**Extended to every single-arm function the file defines (2026-09-13, second cut).** The
first cut derived `defn-` only, on the premise that soundness needed a closed caller set. It
does not. A derived type is a fact about *this file's* calls: a warning in a body walked
under it says "every call in this file would fail here" — true whatever callers exist
elsewhere — and the sharpened return is read only by this file's callers, whose activations
the derived type covers; a caller in another file still reads the demand-based loaded
inference. So a public `days-in-month (y m)` is walked under the `int` its parser hands it,
and the induction runs over activation chains rooted in this file, not over privacy. What
turning it on found: two sites whose scope the collector read wider than the walk —
`and` stores each conjunct in a temporary (`(let (g (int? y)) (if g …))`) and the collector
had no guard alias for it, so `check_let`'s per-binding rule is now one function
(`let_bind_scope`) both walkers call; and a falsy `(or A (nil? root) C)` proves `root` is not
`nil`, the dual of the `and`-conjunct rule the then-branch already had (`or_disjunct_guards`,
each biconditional disjunct's complement on its own variable, no shared variable needed).
Both are general rules; `std/` is at zero in both modes with the derivation on for every
function.

## A list has a positional shape (2026-09-14, ADR-348)

Item 9 of the audit: `(list a b)` was `list<A | B>`, and `(first (list m '(…)))` read
`pair | map` under strict. `Ty` now carries `list_shape` on the pair tag, the sibling of
`tuple` on vector, through the same lattice functions; `(list …)`, a quoted list, `cons`,
`rest` and a `& rest` binder produce it, `first`/`nth`/`count`/destructuring/the oracle/the
runtime contract read it, and the grammar spells it `(list T U …)`. A shape over the node
cap degrades to its element union first, so a long quoted table stays `list<int>`. Also
tightened on the way: the subtype rule's derived element bound covers every sequence member
of the left side (`(tuple int) | pair` claimed `vector<int> | pair<int>` from the tuple
alone). `docs/type-tuples.md` carries the design.

## Recursive types (2026-09-14, ADR-349)

Item 10, the last of the audit's list: a value type that nests itself is `(rec X …)` — a μ
binder on the whole `Ty` and a self-reference term that reads as `any` to what does not
resolve it and as an unknown set to what does; every relation unrolls it first, the two
that recurse coinductively. Inference folds an ascent whose previous value appears inside
its new one into `μX. G[X]` and accepts the candidate only when the next round folds back to
it (a post-fixpoint, so sound); `Ty::widened_below` remains for what neither converges nor
folds. A JSON-shaped decoder reads `(rec X 1 | nil | vector<X>)` where the depth cut read
`vector<… | vector<any>>`. `docs/type-recursive.md` carries the design; deferred there: a
reference across two binders, a named alias, the runtime contract.

## Lengths and indices (2026-09-14, ADR-350)

Item 11, the first of the three Idris took: an interval on the `int` member and a length on
every countable member, in the slots the lattice already had. `count` reads the length,
`rest`/`cons`/`conj`/`range` and the length-preserving combinators move it, int-closed
arithmetic carries intervals (checked — overflow widens, never wraps), and a positional
read is present when the length proves it: `(nth words 1)` under `(>= n 4)`, `(first ms)`
under `(not (empty? ms))`, `(nth xs i)` under `(< i (count xs))` with `i ≥ 0`. Every
fixpoint widens an interval end that moved to its infinity before the ADR-349 fold. The
guards: `empty?` biconditional by length, a comparison between a local, a count and a
literal narrowing both branches, and `(= (nth a k) lit)` a path guard that drops the tuple
alternatives it rules out from the base itself — so a `[:ok x] | [:error msg]` dispatch
types the whole value. On the way: a sealed ability op's domain no longer applies to a
same-file function spelling its name (`tempo/->iso` against `Temporal`, latent), and a
recursive specialization types its self-calls in the branch they sit in (`path/join`'s
accumulator). `(list E)` in the grammar is now `nil | list<E>`. A count relation between
two PARAMETERS is derived from the callers too (`n = (count codes)` handed beside `codes`,
preserved by every self-call), which retired `std/regex`'s last two `check-allow
:type-mismatch` scopes; and a `when`-shaped binding is a guard on its condition. std plain
0, std strict 0, tests plain 0. `docs/type-intervals.md` carries the design.

## `:pure` and `:total` (2026-09-15, ADR-351)

Items 12 and 13, together: a property keyword on the `sig` — `(sig f (int -> int) :pure
:total)`, or `(sig f :total)` alone — registered beside the type in the declared-sig
store. `:pure` walks the body for an effectful head through the functions it calls (a
deny-list, so a finding is an effect the body reaches); `:total` asks every self-call for
a structural decrease read in its branch scope — a shorter non-empty list, a smaller
bounded-below int, a larger int under a literal or a count bound (ADR-350's parameter
relation makes `regex`'s upward loops sayable) — and the walk reports a `match` failure it
cannot prove unreachable. `ui-memo`'s thunk is checked without a declaration, which found
the one deliberate effect in `tests/ui_test.blsp`. `path/join` and the regex DFA loops
carry the first declarations. Also on the way: a `& rest` binder is `nil | list<rest>`
(it was `list<rest>`, unsound for a call with no rest argument), and the site walk binds a
variadic function's parameters from its declared sig. `docs/type-properties.md`.

## Review — stable, and called (2026-09-15)

The audit's thirteen items (ROADMAP "The type system, reviewed") are all shipped:
ADR-341's three cuts, ADR-347 (an arrow in head position), ADR-348 (list shapes), ADR-349
(recursive types), ADR-350 (intervals, lengths, the two count relations), ADR-351 (`:pure`
and `:total`). This entry is the closing review: what was read cold, what it found, what
holds, and what is left.

**Read cold, fixed on the way.** Four things the review found, each with a pin:
- `widen_intervals_against` merged same-tag alternatives on EVERY round; now only when the
  alternatives are multiplying (the divergence signature). A two-branch `[model idx]`
  return is not an ascent, and merging it lost the tuple shape.
- A positional union is exact when one shape is inside the other position-wise, not only
  when at most one position differs — `(tuple m int)` beside `(tuple m int[-1..])` is one
  tuple.
- A positional shape over the node cap keeps its shape with FLAT positions before it
  degrades to the element union: a pair whose model record is deep is still a pair a
  destructuring reads.
- `zlib/` is pure and was on the effect deny-list; the kernel's raw `%write-out`,
  `%getenv`, `%now`, `%random-*` and the clipboard were not on it.

**What holds.** Every relation over-approximates in the direction the checker promises:
hull on union, meet on intersection, checked arithmetic that widens on overflow, a
length never claimed beyond what the shape carries, a read pronounced present only from
a lower bound the lattice holds, a count relation only from sites that all establish it,
an effect reported only when the body reaches a head the list names, a decrease only from
a bound the branch established. `soundness_oracle` runs the corpus; the types suite is
547; `std/` is at zero plain and strict, `tests/` and `examples/` at zero plain.

**Downstream.** bedit's strict ratchet (its own hard gate at zero, against the INSTALLED
`nest`) reads **58** with this checker — and **53** with the checker as it stood before
item 11, so the gate was already behind the checker; the delta is 11 true findings (`first`
of a possibly-empty pane list, in `ed-selected-pane` and ten test sites) and 6 findings
fixed (`git-section-lines`), 8 more only respelled (`nil | int[0..255]`). bedit's ratchet
comment says what to do: fix what the sharper checker found, or raise the ceiling in the
same commit with the reason. That is bedit's commit to make, and `BEDIT_REF` moves with it
(the smoke target's `--bump`).

**What is left**, deliberately:
- ~~`tests/ --strict` holds 37 findings (not a gate)~~ — swept to zero and gated on
  2026-09-17 (the entry below: half were the checker's, one was `math/pow`'s).
- A relation between two locals beyond `i < |xs|`; a `float` interval; a runtime contract
  for an interval or a property; effect inference as a displayed property; totality across
  calls and mutual recursion; a `:total` coverage proof over destructuring patterns (only
  literal patterns are proven today). Each is listed in its ADR's *Deferred* and none has a
  consumer asking.
- Items 5 and 7 above (return-type dispatch, tier-2 monomorphization): unchanged, large.
- ~~Items 5 and 7 above (return-type dispatch, tier-2 monomorphization): unchanged, large.~~ Decided 2026-09-17: item 5 declined (ADR-361); item 7 is a perf item, queued in `perf-handoff.md` with its measurement plan.
## `tests/` to zero strict, and the checker gaps it named (2026-09-17)

The 32 strict findings over `tests/` (the "37, not a gate" of the review above) read as
test code that assumes non-emptiness or handles nil later than the read — and only
about half of them were. The other half were checker gaps a test corpus exposes because
tests hand functions literals, and one stdlib defect no gate could see:

- **`math/pow` had declared nothing for nineteen days.** The adoption batch of
  2026-08-29 inserted its `sig` after the first line of the `defn` — the first line of
  its DOCSTRING — so the file read as declared, `(doc math/pow)` printed the sig as
  prose, and every `(pow int int)` inferred `number` (the chudnovsky port's four
  `math/quot` findings). No gate fails on an absent declaration; `sig_placement.rs`
  now reads every docstring with the real reader (escaped quotes fool a textual count)
  and fails on a column-0 `(sig …)` line — an indented one is a doc example.
- **A declared overload's matching arms now MEET** (ADR-116 addendum,
  `docs/type-arrow-intersection.md`). `pow` declares four arms; under the union the
  `number` catch-all cancelled the `int` arm at every call. `(pow b 3)` reads `int`,
  `(pow 2.0 b)` `float`, `(pow 2 b)` with a possibly negative `b` `number`. A unit base
  is exact under any exponent, and the grammar can say it: `((or (int -1 -1) (int 1
  1)) int -> int)`.
- **A quoted datum handed to `pr-str`/`str` is not an escape.** Every assertion macro
  expands to `(pr-str (quote (assert= … (drive 3000 0))))`, and the derivation read
  `drive` there as a value use — so no private function called from a test was ever
  derived, and a driver derived from its own recursion alone reported `number` on
  `(- i 1)`. Printing a datum cannot call anything; a quoted datum anywhere else still
  escapes (it may be `eval`ed).
- **`record?` is a one-sided guard** (`Ty::implied_by` beside `tested_by`): it holds for
  a record — a map — and fails for a plain map, so the then-branch narrows to `map` and
  the else-branch to nothing; `filter`/`find` keep the narrowing, `reject` does not.
- **Lengths that were dropped.** A literal `(range 1000)` was "non-empty" and lost its
  exact length; `into` onto a vector lost the target's plus the source's; `map` over a
  counted input lost it when the callback's result was unknown (the length is a fact
  about the input). `(nth (into [] (map (range 1000) mk)) 999)` is present now.
- **A mask bounds a conjunction**: `(bit/and x m)` with `m ∈ [0, hi]` is `int[0..hi]`,
  and a computed `nth` index whose interval fits a shape reads the positions it can
  name — `(nth [10 20] (bit/and i 1))` is `10 | 20`.
- **A `:keys`/`:or` binder** lowers to `(get m k default)` — the same semantics as the
  `(if (contains? …) (get …) default)` it replaced (a present nil stays nil), one lookup
  instead of two, and the shape the `get` rule reads exactly: `40 | nil` → `40`.
- **A value sig declared in another file** (`(sig *test-wait-ms* int)` beside the
  `defdyn`) is read by inference through the same heap store a cross-module arrow sig
  comes from; the checking walk already did.

The test-side residue was written honestly: `(or (first rs) (error …))` where a test
assumes a non-empty result, `(get index k {})` where it assumes a key, a tuple compared
whole (`(assert= r [#b"…" #b"…"])`) instead of read position by position, and
`(check-allow :type-mismatch …)` on the one function a test hands `"abc"` on purpose.
`tests/` is a strict gate now, beside `std/` (CI, `make green`, the pre-push hook).

## The site walk, memoised — and the fixpoint that was not one (2026-09-17)

The handoff's "checker cost" item, measured before it was touched. `BROOD_DERIVE_DBG`
now prints per file where the time went: over `std/` (debug), 11.0 s in all, **7.1 s in
the joint fixpoint** (Pass 2.9's derivation walks and return re-inference), and of the
walks' 4.4 s only 0.2 s was typing the sites they collected — the rest was the walk
itself, which builds the scope each site is typed in by re-typing every `let` binding
and every guard in the file, whole, on every round of every joint round.

**`sigs::SiteCache`.** A form's walk depends on exactly two moving things: its own
derived parameters (bound by the `fn` arm) and, through the scope, the returns and
derived parameters of the candidates its body reaches — transitively, because typing a
call re-types the callee's body (`specialize_call`). So the cache keeps each top-level
form's last walk with the own-parameter types it was walked under, and hands it back
when those are unchanged and nothing in the form's reference closure moved; the joint
round reports what moved (returns re-read, parameters re-derived, floors lifted), and
the inner rounds report the parameters they moved. A cached site's captured scope is
re-based on the current file facts when it is read (`Ctx::with_file_of`). Over `std/`,
73% of form walks are reuses; 11.0 → 8.8 s, `tests/` 13.5 → 12.3 s. `BROOD_NO_DERIVE_CACHE=1`
is the A/B lever.

**KI-158, found by the cache's differential.** The first differential disagreed in both
directions, and the disagreement turned out to predate the cache: the joint fixpoint was
not a function of its inputs. The specialization memo outlived the round it was typed
under (a body re-typed under floored returns answered every later round), and the
returns were re-read in `HashMap` order with each applied at once, so the
history-dependent widening landed on either side — the same file settled on `(int 0 2)`
five runs out of six and `0 | 1 | 2` on the sixth. Fixed (the memo cleared per round; the
returns in definition order; the derivation's names sorted), and
`nest::derivation_cache_differential` holds both: `nest check --strict --suggest-sigs`
over `std/` and `tests/`, cache on and off, byte for byte.

## The remaining list — sound first, then as complete as makes sense (2026-09-17)

The criteria are the two the type system is held to: (1) **always sound** — the checker
never claims what is not so, and the runtime contract enforces what the grammar lets a
declaration say; (2) **as complete as makes sense** — a fact the lattice can hold is held.
"Wait for a concrete need" is not a criterion here; ADR-011 still decides language SHAPE
(the declined items are in ADR-361 and the ROADMAP), not checker precision. Ticked here as
each lands; this section is the working list, `handoff.md` points at it.

### A — soundness

- [x] **A1. Intervals enforced at runtime** (2026-09-17). `type-matches?` checks the bound
      of `(int lo hi)` and `(len T lo hi)` — `nil` counts 0, `_` is an open end, a
      non-countable never fits. `tests/contract_test.blsp` § intervals; sabotage reds 6.
- [x] **A2. Recursive types enforced at runtime** (2026-09-17). `(rec X body)` matches
      `body` with `X` standing for the whole (`%rec-unroll`), one level per level of the
      value; an inner binder closes its own name. § recursive type contracts.
- [ ] **A3. `:pure` / `:total` at runtime** — *left for now* (decided 2026-09-17): `:total`
      cannot be checked at a boundary; `:pure` could be and costs; both stay static-only,
      and `type-properties.md` says so.
- [x] **A4. A declared overload is checked against its body per arm** (2026-09-17). The
      body is walked once per arm, its parameters bound as ONE domain among several (the
      guards selecting the other arms are not findings), its return checked against that
      arm's result, a finding the arms share reported once. It found `math/pow`'s float
      arm false at `exp = 0` — `(pow 2.0 0)` answered the int `1` — which the code fixes.
      `refinement::a_declared_overload_is_checked_against_its_body_per_arm`.
- [x] **A5. A trusted declaration is SHOWN as trusted** (2026-09-17). Under `--strict` a
      declared return whose body result is the unknown reports `declared return type T is
      trusted, not verified`; `(check-allow :trusted …)` is the author saying so. The
      `sig!` shim is exempt (it IS the verification). The sweep of 54 sites: 43 fixed at
      the leaf (a record's field types, a `(map K V)` where a bare `map` stood, a
      three-parameter `(map keyword int -> int)` that meant one, `%renames`' value type,
      two `defdyn` value sigs), 11 acknowledged `:trusted` (a kernel message, a table, a
      dispatch table, the assoc-threaded state, `seqable` elements — C11).
      `declarations::a_declared_return_the_body_cannot_verify_is_reported_as_trusted_under_strict`.

### B — determinism and honesty of the tool

- [ ] **B6. A cap that was hit is reported.** `MAX_SPECIAL_FUEL`, `MAX_EXPR_MEMO`,
      `MAX_DERIVE_ROUNDS` (NO FIXPOINT), `MAX_EXPR_TY_DEPTH`, the widening — each declines
      soundly and none says so, so "zero warnings" can mean "gave up". A summary line
      naming the file and the cap.
- [ ] **B7. A stale binary cannot check silently.** `nest check` resolves a `:use`d std
      module from the binary's baked-in std; a `nest` built before a sig edit checks
      against the old declaration. Refuse, or warn on every run, on a stdlib-id mismatch
      between the binary and the tree.
- [ ] **B8. An order-dependence audit.** KI-158 found two hash-order channels by accident.
      One deliberate pass over the checker's `HashMap`/`HashSet` iterations that feed a
      verdict, and a second differential gate: the same list in shuffled order, and two
      runs, must agree.

### C — completeness

- [ ] **C9. Subtyping across union terms.** A term covered jointly by two of the other
      side's terms but by neither alone reads "not a subtype" and defers (ADR-262). ADR-289
      closed products and ADR-292 arrows; the general case remains.
- [ ] **C10. The merely-wider residue, re-probed under intervals.** A body typed `number`
      declared `int` is silent by design where undecidable; with ADR-350's intervals more
      of it decides (`quot`, `floor`, a masked value). Flag what is provable, keep the rest
      silent.
- [ ] **C11. Element-typed `seqable`.** The `elem` refinement stops at `pair | vector`; a
      `seqable` parameter carries no element type.
- [ ] **C12. Relations between two locals beyond `i < |xs|`** (`i < j`, `i + 1 ≤ |xs|`).
      A relational domain is a different lattice; do the shapes the corpora show, not the
      domain.
- [ ] **C13. A `float` interval.** Cheap on the int one's machinery; do it with C10 if C10
      needs it.
- [ ] **C14. A named recursive alias** — `(deftype json (rec …))` so a `sig` names it once.
      Nested self-reference across binders stays out (de Bruijn for a shape inference
      never produces).
- [ ] **C15. Effects displayed; totality across calls.** The walk computes a function's
      effects and shows them nowhere (`nest docs`, hover). Totality across calls and mutual
      recursion is a call graph with a measure per edge.
- [ ] **C16. The small holes the 2026-09-17 sweep noticed.** A string source gives `into`
      no length; a closed record's `count` is not its field count; a computed `nth` index
      can be bounded by its interval but not by a guard (only a local can).
- [ ] **C17. Dispatch on a type designator** — the door ADR-361 leaves open: a multimethod
      keyed on `(zero-of :int)`'s argument as a designator. Language-side; this one IS a
      use-case item.

### Off this box

- [ ] Tier-2 monomorphization — `perf-handoff.md` Task 5.
- [ ] The bedit `--bump` smoke — needs a box that runs the full suite.

## Sound first: A1, A2, A4, A5 (2026-09-17)

The first pass through the list above, in its order. What each found is the point.

- **The contract now holds what the grammar can say.** `(int 0 255)`, `(len (list E) 1 _)`
  and `(rec X …)` parsed in a `sig!` and were checked at the tag: a declaration the
  checker trusted could lie at run time. `type-matches?` checks the bound and unrolls the
  binder; 7 new contract cases, 6 of which red under sabotage.
- **A declared overload is checked per arm — and its first finding was mine.** `math/pow`'s
  `(float int -> float)` arm was false at `exp = 0`: `(pow 2.0 0)` answered the int `1`,
  because the accumulator was seeded `1` whatever the base. The code now seeds the base's
  kind, `pow-acc` and `pow-reciprocal` declare their arms, and the unit-base arm went (the
  reciprocal path cannot prove it; chudnovsky's alternating sign is written as the sign).
  A wrong arm is no longer trusted at every call the meet reaches.
- **A trusted declaration is shown.** Strict reports `declared return type T is trusted,
  not verified` where the body's result is the unknown. Over `std/` that was 43 sites;
  reading each one: 32 were the checker's or the declaration's — a record's field types
  never declared (`span`, `tempo`), a shape never named for a node (`ts-node`,
  `sexp-node`, `coverage-result`), a bare `map` where `(map keyword int)` was meant, a
  three-parameter `(map keyword int -> int)` that meant one, `%renames` typed as a bare
  map, two `defdyn`s with no value sig, a `check-allow`-wrapped `defn` invisible to
  inference, the regex parser chain declared `number` for its index because the checker
  could not carry `int` through a destructured self-call — and 11 are genuinely beyond
  it, acknowledged with the new `(check-allow :trusted …)`: a kernel message
  (`read-line`), a table (`regex-exit`, `regex-first-mask`, `resolver-step-count`), a
  dispatch table (`nest/main`), assoc-threaded state (`markdown/->html`, `pane-update`),
  `seqable` elements (`stats/min`/`max` — C11), the CST's kids (`forms-of`), and an open
  record's undeclared key (the `datetime` time accessors over a `date`).
- **Four inference holes closed on the way**, each a fixpoint that started at the unknown
  instead of ⊥ and could never come back: a destructuring of a `never` bound its names
  unknown; a numeric op on a `never` deferred to its signature (`number`); an `if` whose
  test is `never` read as the union of nothing; a self-call not in branch-result position
  read as unknown in its own first round. And one relation defect: `(int and (not 0))`
  read as "known only by exclusion" because `as_lit_int` of `0 | (not int)` answered
  `{0}` — strict read such a value by overlap, and merely-wider misuses of it went
  unreported. The widening of a moving interval beside a stable tuple of the same tags,
  the string literal's length, and `pattern_bindings` over `never` are the other three.
