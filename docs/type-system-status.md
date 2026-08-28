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
