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
| 5 | **Return-type dispatch** — selecting an impl by expected return | Needs bidirectional inference. The long-standing open item in [protocol-dispatch-design.md](protocol-dispatch-design.md) | Large |
| ~~6~~ | ~~Qualified cross-module ability type names~~ | **Shipped 2026-08-29** — and the headline was already done: in a PROJECT check both `Shape` and `shapes/Shape` resolve, because `ability_type` reads the last `/` segment (the registry is keyed by bare CamelCase name, ADR-255). What was broken was the loose single-file fallback, where neither can resolve and the checker falls back to "capitalised means an ability I cannot see" — that test read the WHOLE spelling, so `shapes/Shape` reported `unknown type` while bare `Shape` was accepted. Naming the module an ability comes from must not be what manufactures a diagnostic | ✅ |
| 7 | **Tier-2 monomorphization** — devirtualizing an *inferred-variable* op call | Still deferred, and now for a better-founded reason. Turning `BROOD_MONO` on for the first time (2026-08-29 — **nothing in the repo had ever set it**) found Tier 1 miscompiling: it baked the resolved impl *value*, and a body compiles before it runs, so a module registering an impl and using it in the same body called the wrong one. Fixed by proving the identity and leaving resolution behind the epoch-guarded cache (ADR-294), and gated by a differential. Tier 2 multiplies that surface across every call site the checker can type and still needs the checker→compiler channel — but it now has a sound base and a gate that catches a miscompile the first time the flag goes on | Large |
| ~~8~~ | ~~Runtime contracts for ability ops~~ | **Shipped 2026-08-29** (ADR-293): `impl` wraps a method whose op declares `:-> RET`, decided at expansion time so an unset flag emits nothing. Building it revealed `BROOD_CONTRACTS=1` had rotted into *unusable* — three cold-boot-cache-only defects, none of which any gate could see, because the mode had no end-to-end test at all (KI-81) | ✅ |
| 9 | **`sig` adoption across std** — **407** declarations over **2942** `defn`s (369 earlier on 2026-08-29; 34/2828 on 2026-08-28) | Every one now buys more than it did: a declared sig is what the reversed-args gate (KI-71) reads, and inference is checked against it (ADR-259) | Ongoing |

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

