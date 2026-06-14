# Brood types — set-theoretic, gradual, advisory

**Status:** steps 0–4 done; Step 5+ well underway (ADR-078) — function **arrow**
types, sequence **element** types, **parametric HOF results**
(`map`/`filter`/`reduce`/`fold` flow element types through), **`(and …)`
intersections**, **`(map K V)` key/value contracts** (runtime + checker
refinements for `get`/`keys`/`vals`/`assoc`), and **`?A` type variable
unification** (user-declared sigs: `SigTerm`/`SigWithVars` resolve return
types per-call from arg types) all shipped. The
advisory checker (`(check 'form)`)
is the lattice's first consumer (`crates/lisp/src/types/{mod,check}.rs`). This doc is the
plan *and* the compatibility contract: the staircase says what to build next, the
[Compatibility contract](#compatibility-contract) says what every other change
must respect so we never drift off this path. Decision recorded in
[ADR-024](decisions.md) (refining [ADR-023](decisions.md)).

## The decision, in one paragraph

Brood's types follow the **Elixir model — set-theoretic and gradual** — not
TypeScript's pragmatic-but-unsound one. A type *is a set of values*; subtyping is
set inclusion; what can't be pinned down statically is `dynamic()` and mixes
soundly with the rest. Checking is **advisory**: it warns and optimises, it never
rejects a runnable program (the one exception — provably-sound special-form
*structure* errors — can't be wrong because special forms aren't redefinable).
The language stays fully dynamic; types never inhibit it. Mechanism lives in Rust
(`Ty`, the `Tag` atoms, primitive signatures); policy (`assert-type`, contracts)
lives in Brood (ADR-006).

Reading: Castagna, Duboc, Valim, *"The Design Principles of the Elixir Type
System"* (‹Programming›, 2024; on arXiv) and the semantic-subtyping / set-theoretic
lineage behind it.

## The model

A `Ty` **is a set of values**, and the type operations *are* set operations:

| Type op | Set op | In `types/mod.rs` |
|---|---|---|
| union (`int \| float`) | `∪` | `Ty::union` (bitwise OR) |
| intersection | `∩` | `Ty::intersect` (AND) |
| negation ("not nil") | `¬` | `Ty::negate` (complement) |
| **subtyping** | `⊆` inclusion | `Ty::is_subtype` — *semantic*, no syntactic rules |

- **Atoms** are the 17 runtime [`Tag`](../crates/lisp/src/core/value.rs)s
  (`int float string symbol keyword bool nil pair vector fn macro native map ref
  pid rope socket`). The type universe is built from these; `type-of` observes one
  at runtime. Function members can additionally carry a structured *arrow*
  refinement (Step 5+, ADR-078).
- `Ty::NEVER` = `⊥` (empty set, subtype of everything); `Ty::ANY` = `⊤` (all
  tags); the named unions `Ty::NUMBER` (`int∪float`), `Ty::LIST` (`nil∪pair`)
  match the `number?`/`list?` predicates.
- **`dynamic()`** *(step 2, `GradualTy`)* is the **gradual** type — and it
  lives *inside* the set-theoretic algebra, not bolted beside it. It's a bounded
  type `dynamic(bound)` (pure `dynamic()` = `dynamic(ANY)`) whose `bound` is an
  ordinary set-of-tags `Ty`, read as the interval between its optimistic (`⊥`)
  and pessimistic (`⊤`) materialisations. Crucially, **consistent subtyping is
  *derived from* ordinary set inclusion** — not a separate, non-set "consistency"
  axiom (the classic Siek–Taha framing). For our flat lattice the derived rule is
  simply: `dynamic(b)` is consistent-compatible with `t` iff `b ∩ t ≠ ⊥` (some
  materialisation fits); static-vs-static stays plain `<:`. So `dynamic()`
  composes with `∪`/`∩`/`¬` like any type and honours [contract point
  #2](#compatibility-contract). Anything whose type can't be pinned — above all a
  **redefinable global under hot reload** — is `dynamic()`, **not** `ANY` (`ANY`
  relates by subtyping and *would* error when an `int` is wanted; `dynamic()`
  defers). This is the valve that lets typing coexist with live redefinition.
  (Castagna & Lanvin, ICFP 2017; Castagna et al., POPL 2019 — the reconciliation
  Elixir uses.) **Note:** the advisory *checker* (Step 4) doesn't use `GradualTy`
  — it carries `Option<Ty>` (known / unknown). `dynamic()` is foundation for a
  later gradual-*assignment* checker, not the disjointness pass.
- **Structured types** arrive as refinements on the flat lattice (Step 5+, all
  shipped — ADR-078): a **function arrow** `int -> int` (the `arrow` refinement)
  and a **sequence element type** `vector<int>` (the `elem` refinement), with
  `map`/`filter`/`reduce`/`fold` results derived from their arguments. The flat
  tag bitset remains the coarse set under any refinement.
- **Keyword-literal (singleton) types** (the `lit` refinement, ADR-105): a sig can
  enumerate exact keyword values — `(or :maximized :fullboth nil)` — and the checker
  flags a keyword outside the set. Unlike the other refinements, union is *exact*
  (the set-union, since `(or :a :b)` is precisely both), and `is_disjoint` gains a
  precise keyword case so `:c` is provably-not-`(or :a :b)` — still sound (a literal
  set is an enumeration, never an over-approximation). Keyword-only so far.

## The staircase — tackle one at a time

Each step is self-contained, ships green, and is useful on its own. "Done when"
is the checkable boundary.

### Step 0 — runtime tags first-class ✅ (ADR-023)
First-class `Tag` + `(type-of x)`, self-identifying type errors, and an `Arity`
on every builtin enforced at one gate (`eval::call_native`).
**Done:** tag is observable; errors name op/wanted/got; arity is metadata.

### Step 1 — the set-theoretic `Ty` lattice ✅
`crates/lisp/src/types/mod.rs`: `Ty` as a set of tags with union/intersect/negate/
difference, semantic subtyping, `NEVER`/`ANY`/`NUMBER`/`LIST`, `of_value` bridge,
`Display`. Pure algebra; nothing in the language consumes it yet.
**Done:** the algebra exists and is unit-tested in isolation.

### Step 2 — `dynamic()`, the gradual type ✅
`types/mod.rs`: `GradualTy { bound: Ty, dynamic: bool }` — `dynamic(bound)` kept
*inside* the lattice (pure `dynamic()` = `dynamic(ANY)`). `consistent_with` is
**derived from set inclusion** (static → `bound ⊆ expected`; dynamic → `bound ∩
expected ≠ ⊥`), not a primitive consistency axiom — so pure `dynamic()` is
consistent with every inhabited type while `dynamic(number)` is still caught
against `string`. Joins branch types via `union`; gradual `intersect`/`negate`
are deferred until a consumer needs them (ADR-011 — don't ship unproven
operators). The "redefinable/free/global references are `dynamic()`" rule is
documented (the struct doc + ADR-024); no checker consumes it yet.
**Done:** the gradual type and its derived relation exist and are unit-tested.

> **Status note:** `GradualTy`/`consistent_with` now have their **first consumer** —
> the **gradual-assignment check** on `(def x …)` against a non-arrow `(sig x T)`
> declaration (`walk::gradual_of` + `check_def`). The *disjointness* pass over
> `Option<Ty>` is still its own thing — a pure disjointness check genuinely doesn't
> need `GradualTy` (an unknown is silent, which is `dynamic()`'s behaviour for free),
> so it stays as-is. The gradual machinery earns its place precisely where
> disjointness can't reach: an *assignment* uses **consistent subtyping**, and a
> reference to a redefinable global with a declared type is `dynamic_within(t)` — a
> *bounded dynamic* `Option<Ty>` (only known/unknown) structurally cannot represent.
> So `(def count label)` with `label : string` and `count : int` is flagged (the
> bounds are disjoint), while `(def count maybe-int-global)` defers (hot-reload
> safe). See [`research/set-theoretic-types-in-brood.md`](research/set-theoretic-types-in-brood.md).

### Step 3 — signatures the checker reads ✅
A callee's signature (argument `Ty`s + result `Ty`) comes from three sources,
simplest-first — deliberately **no inference engine** (see the rationale in
[How it runs](#how-it-runs--and-why-its-outside-the-runtime)):

- ✅ **Primitives** — every [`NativeFn`](../crates/lisp/src/core/value.rs)
  carries a [`Sig`](../crates/lisp/src/types/mod.rs) field next to its `Arity`
  (compatibility-contract point #6, **enforced** — there's no way to construct
  a `NativeFn` without one). The checker reads it via a global-env lookup
  (`check::primitive_sig`); there is no parallel hand-maintained table.
  Primitives whose args/result aren't usefully pinned use the explicit
  `Sig::any()` lane (`(...any) -> any`) — overlaps every input, so the
  disjointness checker never warns against it.
  Example sigs: `%add: (number,number)→number`, `first: (list|vector)→any`,
  `string-length: (string)→int`, `string->number: (string)→number|nil`.
- ✅ **Curated stdlib** — a small hand-written table for the variadic /
  `reduce`-based / branchy / higher-order Brood closures the checker can't infer
  but that matter: `+ - * / < <= > >= mod`, `map`, `filter`, `reduce`, `fold`,
  plus common helpers with branchy or nested-param bodies — `even? odd? abs`
  (numeric), `not zero?` (any → bool, for the result type), `count length`
  (string|map|seq → int). Hand-vetted against `std/prelude.blsp`, so sound; the
  domains are kept to the widest type the body accepts so a tighter sig never
  false-positives. This is what makes `(+ 1 "x")` and `(even? "x")` catchable
  even though both are plain Brood closures.
- ✅ **Basic inference** (`check::infer_sig`) — *only* for a fn whose body is a
  **single straight-line expression** (no `if`/`cond`/`when`/`let`/`match`/
  recursion, no `&optional`/rest params): each closure parameter inherits the
  type the callee expects at the position(s) where the parameter is used
  directly (intersected across positions); the closure's return is the
  callee's. Anything with a branch / binding / recursion → infer nothing.
  Sound **because a straight-line use is unconditional** — no control-flow
  analysis, no fixpoint, no false-positive class. The callee is itself only
  looked up via the *non-inferring* `primitive_sig`/`curated_sig` (so a chain
  `defn a (x) (b x)` / `defn b (x) (a x)` can't loop). Catches one-liner
  wrappers (`inc`, `twice`, simple user `defn`s); skips everything subtle.

**Deferred (⬜):** inference through branches / guards / recursion / higher-order.

### Step 4 — the advisory checker 🟡 (v0 shipped; plan below)
`crates/lisp/src/types/check.rs`: walk a macro-expanded form and **warn when a
call passes a provably-wrong argument** — its type is *disjoint* from what the
callee accepts (`(first 5)`; `(+ 1 "x")` once `+` has a curated sig).
Disjointness (not subtyping) is the rule, so a superset / unknown argument is
never a false positive.

- **Vocabulary: `Option<Ty>`, not `GradualTy`.** The checker only asks "do I know
  this argument's type?": `Some(t)` → check disjointness against the param;
  `None` (a variable, an unknown call) → stay silent. The gradual machinery
  isn't needed until we check *assignments*; the disjointness checker doesn't, so
  it stays out of the hot path.
- **Skip inside `try` / `error-of` / `assert-error`** — those forms deliberately
  exercise failures, so don't flag code within them (keeps `nest test` quiet on
  error-path tests).
- **Advisory, always** — returns warnings; never raises, never gates (contract #5).
- ✅ **v0 shipped:** the `(check 'form)` builtin + `brood --check <file>`
  (located warnings).
- ✅ **Step-3 coverage:** primitive sigs sourced from `NativeFn` (enforced;
  no parallel table), curated stdlib sigs for `+`/`<`/`map`/…, and inference
  for straight-line single-expression closures (so a user `(defn inc (x) (+ x
  1))` participates without a hand-written sig).
- ✅ **Guard narrowing + let-binding tracking** (the second behavioural payoff):
  the checker now threads a `Ctx { sym → Ty }` of locally-known types through
  the walk. A `let`/`let*` binding seeds the variable with the RHS's
  `expr_ty`; an `if`'s test narrows in both branches via [`Ty::tested_by`]
  (`(if (int? x) … …)` ⇒ in the *then* branch `x` is `int`, in the *else* it's
  `not int`); `(not <inner>)` flips. Inner shadowing overrides — a fresh
  binding to an unknown RHS *removes* an outer narrowing rather than
  intersecting (otherwise the outer leaks through the shadow).
- ✅ **Let-bound guard aliases.** `(let (cond (int? x)) (if cond …))` now
  narrows `x` (not the bool `cond`) inside the if. The `Ctx` carries a second
  table `guards: sym → (var, asserted-ty)`; a `let` records the alias when
  the RHS is itself a recognised guard, and `guard_assertion` on a bare `Sym`
  test looks it up. Sound because Brood is immutable — between the let and
  the if neither `x` nor `cond` can change. Self-aliasing (`(let (x (int? x))
  …)`) is rejected (the outer `x` is shadowed).
- ✅ **Let-binding aliases + `%eq` guards** — the pair that closes `match`
  pattern narrowing. The `match` pattern compiler lowers `(match x (5 body)
  …)` to `(let (m__N x) (if (%eq m__N 5) (do body) …))`; `body` references
  `x` (not the internal `m__N`), so narrowing has to flow back. Two pieces
  do it: `Ctx.aliases: HashMap<Symbol, HashSet<Symbol>>` records the
  undirected `(let (a b) …)` equivalence between a name and another symbol,
  and `narrow_chain` BFSes the equivalence class on every narrow so an
  assertion on either side propagates to the other. The guard recogniser
  learns `(%eq sym lit)` (and the symmetric `(%eq lit sym)`) as an assertion
  `sym : type-of(lit)` — covering literal-int, -keyword, -string, -bool, and
  -nil patterns. With both in place, `(match x (5 (first x)))` now flags
  `first: argument 1 expects nil | pair | vector, got int (x)`. `shadow`
  fully disconnects a name from the alias graph (its bin removed and the
  name pruned from every neighbour's bin) so a rebinding doesn't leak
  through stale back-edges. Sound for the same immutability reason as guard
  aliases. (Cond / and / or didn't need any new machinery — `cond`'s direct
  `(pred? sym)` tests and `and`/`or`'s gensym `let`-then-`if` expansion are
  already handled by the existing guard pipeline.)
- ✅ **Arity diagnostics.** Every call's argument count is checked against the
  callee's `Arity` — `NativeFn.arity` for primitives, derived from
  `Closure.{params, optionals, rest}` for Brood closures (in the heap; the
  inferred-sig path applies too). `(first)` → "expected 1, got 0"; `(rem 1 2
  3)` → "expected 2, got 3"; `(map-get {})` → "expected 2 to 3"; `(apply f)`
  → "expected 2 or more". Independent of the type check (which still runs
  for the args that *are* present), so a 1-arg `(first 5)` still says `first:
  argument 1 expects nil | pair | vector, got int (5)`.
- ✅ **Unbound-symbol diagnostics** (call heads). A call whose head doesn't
  resolve to *anything* — not a primitive, not a curated stdlib closure, not
  in local scope (fn-param, let-binding), not a file-local `def`/`defn`/
  `defmacro`/`defdyn`, not a syntactic keyword (`if`/`do`/`when`/`cond`/`and`/
  `or`/`match`/`->`/…), and not in the heap's global table — is flagged
  `unbound symbol: foo`. The walk gained scope-aware handling of `fn` /
  `lambda` / `def` / `defn` / `defmacro` so binders aren't seen as references
  and fn-params get bound into `Ctx`. A new `check_file(heap, &[forms])` API
  threads top-level `def`/`defn` names across forms so a later call to an
  earlier definition isn't flagged. The CLI's `brood --check` now uses
  `check_file`.
- ✅ **Function-as-value lint** (advisory). A bare reference to a *known
  zero-arity* global passed to an output sink (`print` / `println` / `str` /
  `format`) — e.g. `(print ansi-clear)` where `(print (ansi-clear))` was meant —
  is flagged `ansi-clear: function used as a value — did you mean (ansi-clear)?`.
  Catches the otherwise-silent slip where a zero-arg helper stringifies as
  `#<fn …>` instead of being called. Restricted to those sinks and to *globals*
  (a same-named local is left alone) to stay false-positive-free.
- ✅ **Operand-position unbound check.** The unbound diagnostic now fires on
  *operand / value* positions too — `(+ 1 typo)`, `(def x typo)`, `(if typo …)`,
  `(let (a typo) …)` — not just call heads. A bare-symbol operand is flagged only
  when its enclosing head is a proven *arg-evaluating, non-macro* callee (a
  primitive, a curated/known closure, or a lexical local — `evaluates_args` in
  `check/walk.rs`), so an unexpanded macro argument is never mistaken for a value
  reference. It is further gated to **whole-file mode** (`check_file` only): there
  every top-level def is accumulated and the project image is loaded, so an
  unresolved operand is genuinely unbound — whereas a bare fragment (`(check
  'form)` / a REPL snippet) keeps free operand variables ambiguous and flags only
  the head. Both checks reuse the one `is_unbound` predicate, so they can't drift.
  Audited over the whole `std/` + `tests/` tree: **zero** false positives.
- ✅ **Auto-running at file boundaries.** The checker now fires automatically:
  `brood <file>` and `brood --test <file>` pre-check before evaluating (CLI
  wiring through `check_one_file`); `nest run` and `nest test` pre-check the
  whole project after loads but before running (Brood `(check-project)` in
  `std/tool/project.blsp` walking every `.blsp` under `src/` + `tests/`). Warnings
  go to **stderr** so they don't muddle program output; the run/test
  **proceeds regardless** (advisory, never gates — `contract #5`). Set
  `BROOD_NO_CHECK=1` to opt out (e.g. when timing a hot path).
  Mechanism: a new `(check-file path)` Rust primitive reads and checks a file,
  returning pre-formatted `path:line:col: warning: …` strings; policy in Brood
  iterates over the project's files via `(check-project)` (the standard
  policy-in-Brood pattern, ADR-006).
- ✅ **Macro-expansion before walking.** `check_file` now macroexpands each
  top-level form before walking it, so threading macros (`->`/`->>`), pattern
  syntax (`match`), test framework wrappers (`test`/`describe`/`error-of`/
  `assert-error`), and any user macro that rearranges code are checked
  against their *expanded* shape — eliminating false positives like
  `(map inc)` inside `(->> xs (map inc))` getting flagged as 1-arg. The
  file-globals accumulator likewise walks the expanded tree recursively, so
  a `(defn foo …)` nested inside `test`/`describe`/etc. still shields a later
  `(foo …)` from the unbound check. Positions survive expansion where the
  macro rebuilds through `rebuild_list` (the common case).
- ✅ **Cond / match / and / or guard narrowing all in.** `cond` flows
  through `if`'s existing `(pred? sym)` recognition; `and` / `or` through
  the `let`-stored guard-alias path (the prelude expansion `(let (g a) (if
  g b g))`); `match` through the new let-binding alias + `%eq` guard. The
  whole Step-4 surface is behavioural now — every form a user reaches for
  on a guarded variable narrows it.
- ✅ **Macro-hygiene lint** (`check/hygiene.rs`). Macros are unhygienic by
  default (ADR-021/no auto-rename), so a `defmacro` template that introduces a
  binder with a *literal* symbol can **capture** caller code spliced into that
  binder's scope — the `(defmacro time (expr) ` `` `(let (start (now) v ~expr) …)) `` ``
  bug, where the body's `start` binds to the clock instead of the caller's.
  The lint warns only when **both** hold for a `let`/`fn` binder inside a
  quasiquote template: (1) the binder is a literal symbol — a gensym'd binder
  reads as `(unquote g)` and an unquoted caller-name as `(unquote evar)`, so
  neither trips it; and (2) a macro *parameter* is spliced (`~p`/`~@p`) into
  that binder's scope (Brood `let` is sequential, so the scope is the body plus
  *later* bindings' values — not the binder's own value). Both conditions are
  syntactic, so this is the one pass that runs over the **un-expanded** forms
  (templates vanish after expansion). Audited over the whole `std/` tree: every
  macro there gensyms or unquotes its binders, so the lint fires **zero** false
  positives (contract #5 holds — advisory, never gates). An intentional
  anaphoric macro (deliberate capture) would be flagged; none exist in-tree, and
  if one is written the lint should grow an opt-out rather than relax the gate.
- ✅ **Protocol / behaviour conformance** (`check/protocol.rs`). Beyond type
  misuse, the checker validates the `defprotocol` / `defbehaviour` / `defimpl` /
  `(:implements …)` family against the declared interface: a diagnostic per op an
  impl omits, per op whose arity disagrees, and per method the protocol never
  declared (almost always a typo). Behaviours additionally check that a module
  claiming `(:implements Name)` actually *defines* each declared op at the right
  arity. The interface registry is seeded from the runtime `*protocols*` map (so
  an interface declared in an imported module is known) and read from the
  **un-expanded** tree (the protocol shape vanishes after `defprotocol` lowers to
  `defn`s + registry calls — the same reason `sig` and the hygiene lint read
  un-expanded). An impl/claim of an *unknown* protocol is left alone (it may be
  declared in a file this one doesn't import) — no false positive.
- ✅ **Non-tail self-recursion lint** (`check/recursion.rs`). A `defn` whose
  self-call sits in a non-tail position is flagged — deep non-tail recursion
  overflows the small green-process coroutine stack (today an uncatchable
  segfault, not a clean error). Advisory; the fix is a tail-recursive accumulator
  or a process-driven loop.
- ✅ **Dead-clause lint** (`walk::newly_dead_sig_param`). When a `(sig …)` pins a
  parameter's type, a `match` / `cond` clause whose guard narrows that parameter
  to the empty type (`NEVER`) is provably unreachable and flagged. Sound because
  it only fires on a *declared* sig type intersected to `⊥` — never on an inferred
  or unknown type.

With everything above, Step 4 is **done**, including the operand-position
unbound check and a single unified `nest check` path (whole-project *and*
file-list checks both load the project image first via Brood `project/check-files`
/ `check-project`, so cross-namespace imports resolve identically — no second
code path). The only meaningful next move is the upgrade to Step 5+ (structured
types) when a real need surfaces.

### Step 5+ — structured types 🟡 (arrows + element types shipped; ADR-078)
Function arrows, vector/list element types, intersections for overloaded fns —
the fuller set-theoretic algebra. Additive; gated on real need (ADR-011).

**✅ Function arrows (first slice, ADR-078).** `Ty` is now a **refinement struct**
`{ tags: u32, arrow: Option<Arc<Sig>> }`: the flat tag bitset stays the coarse set
(the whole pre-Step-5 behaviour, verbatim), and `arrow` refines the function members
(`Fn`/`Native`) to a specific signature when known — an arrow type *is* a [`Sig`].
So `(int) -> int` is `{tags: Fn|Native, arrow: Some((int)->int)}`; a bare "any
function" is the same tags with `arrow: None`. This **refines** the bitset rather
than *replacing* it with the originally-sketched `enum { Set, Arrow, Vec }` — a
union across kinds (`int ∪ (string -> int)`) is then just the tag union plus a
per-kind refinement, sidestepping the DNF-of-frames an enum would force (the
expensive part ADR-011 says to defer). Arrow subtyping is contravariant in
parameters / covariant in the result (`Sig::is_subtype`); the set ops only ever
*widen* a refinement toward `None` when they can't represent the exact result, and
`is_disjoint` ignores arrows entirely — so a refinement can only suppress a warning,
never raise a false one (contract #5). **The payoff:** `map`/`filter` carry a 1-ary
callback arrow and `reduce`/`fold` a 2-ary one, so the checker flags a callback of
the wrong arity — `(map cons xs)`, `(reduce (fn (a) a) 0 xs)` — whenever the
callback's arity is knowable (a named global fn, or a simple lambda literal);
unknown arities are skipped, so zero false positives across `std/` + `tests/`.

**✅ Element types (second slice, ADR-078).** `Ty` gained a second refinement,
`elem: Option<Arc<Ty>>`, refining the sequence members (`pair`/`vector`) to their
element type — `vector<int>` is `{tags: Vector, elem: Some(int)}`. Sources: a vector
literal `[1 2 3]` and the `(list …)`/`(vector …)` constructors get the union of their
element types (any unknown element → unrefined). Sinks: `(first xs)`/`(last xs)`/
`(nth xs i)` flow the element type out (widened with `nil` for the empty/out-of-range
case), so `(+ 1 (first ["a" "b"]))` is flagged (`string | nil` is disjoint from
`number`) while `(first [1 2 3])` stays a number. Element subtyping is covariant
(sound — Brood sequences are immutable); union widens on a mismatch; `is_disjoint`
stays tags-only. Surfacing precise sequence types exposed a latent guarded-use gap —
the `match` compiler's vector-pattern lowering `(if (and (vector? m) …) (… (vector-ref
m i) …) …)` — so `guard_assertion` now narrows through the **`and` short-circuit
shape** `(let (g E) (if g _ g))` (the first conjunct holds in the then-branch; `or`'s
`(if g g _)` deliberately does not), which keeps the guarded `vector-ref` quiet. Zero
new false positives across `std/` + `tests/`.

**✅ Parametric HOF results (third slice).** `map`/`filter` results now carry an
element type derived from their arguments — `(map f vector<A>) : nil | list<B>`
where `B` is the callback's return (a named fn's sig result, or a straight-line
lambda's body typed with its parameter bound to `A`), and `(filter pred coll)`
preserves `coll`'s element type, and `reduce`/`fold` give an accumulator typed
`ty(init) | B` (`B` = the 2-arg callback's return, accumulator over-approximated as
`any`). Done as **per-HOF result rules** in `seq_aware_call_ty` (Option B — no
lattice change, the same place `first`/`list` derive a refined result), not type
variables. So `(first (map inc [1 2 3])) : number | nil` and `(reduce + 0 [1 2 3]) :
number` flow through. Uncertain callback / element → flat fallback (sound;
`is_disjoint` stays tags-only). See [`parametric-result-types.md`](parametric-result-types.md).

**✅ Structural combinators (fourth slice).** Element types now flow through
`reverse`, `sort`, `sort-by`, `take`, `drop`, `take-while`, `drop-while`, `cons`,
`append`, and `concat` — the structural combinators that reshape a sequence without
transforming its elements. `(reverse vector<int>) : nil | list<int>`, `(take 2
list<string>) : nil | list<string>`, `(cons 1 list<int>) : list<int>` and so on.
`sort`/`sort-by` treat the sequence as the last argument (both 1-arg `(sort xs)` and
2-arg `(sort f xs)` forms). `cons` requires both the head type *and* the tail element
type to be known (either unknown → unrefined `pair`). `append`/`concat` union the
element types of all arguments; any argument with an unknown element type → flat
fallback. Zero new false positives across `std/` + `tests/`.

**✅ Structural combinators (fifth slice).** The element-type flow was extended to
the rest of the element-preserving / element-extracting sequence library:
`second`/`third` (extract — `A | nil`, like `first`); `rest`/`but-last`/`distinct`/
`dedupe`/`take-last`/`drop-last`/`remove` (preserve — `nil | list<A>`, like
`reverse`/`take`); `keep` (`map`-then-drop-`nil` — `nil | list<B>`, like `map`);
`interpose` (`nil | list<A | type(sep)>`, weaving the separator in); and `range`
(`nil | list<number>` — always numeric, a sound superset over int/float bounds).
So `(+ 1 (first (rest ["a" "b"])))` and `(string-length (first (range 5)))` are now
flagged. Each rule yields the *exact* element type or a sound superset, and
`is_disjoint` still ignores element refinements, so the additions can only sharpen a
result — never raise a false positive. Zero new across `std/` + `tests/`.

**All previously-deferred items shipped (ADR-011).**

- ✅ **Expanded curated sigs** — shipped: predicate group (`number?`/`empty?`/`list?`/
  `contains?`/`member?`/`some?`/`every?` → `bool`) and string-converter group
  (`symbol->string`/`join`/`string-capitalize`/`string-split` → `string`/`list`).
  Catches `(+ 1 (number? x))`, `(+ 1 (join …))`, etc.
- ✅ **Rest/variadic in `(sig …)` annotations** — shipped: `(sig f (int & number -> int))`
  wires `Sig::rest` and the `sig!` macro generates a rest-checking wrapper.
- ✅ **`sig!` runtime enforcement** — shipped: `sig!` wraps the target function with a
  per-argument and per-result runtime check; `BROOD_CONTRACTS=1` enforces every
  `(sig …)` the same way. See `docs/type-annotations.md`.
- ✅ **Inference through simple let-aliases** — shipped: `infer_sig` now peels a single
  `(let (alias param) call)` wrapper via `unwrap_let_alias`, so `(defn f (x) (let (y x) (+ y 1)))`
  infers `number -> number`. Multi-binding or computed RHS lets are not peeled (sound).
- ✅ **Intersections** `(and TypeA TypeB)` — shipped: `type-matches?` handles `(and …)`
  via `every?` (one line); `parse_type` in `annot.rs` produces `Ty::intersect` for
  the static checker. See [`docs/type-intersections.md`](type-intersections.md).
- ✅ **Map key/value types** `(map K V)` — fully shipped: `type-matches?` walks
  `entries` for runtime contracts; `Ty::map_of` carries `map_kv` refinement in
  the checker; `get`/`keys`/`vals`/`assoc` derive precise result types.
  See [`docs/type-map-kv.md`](type-map-kv.md).
- ✅ **Type variables** `?A` — fully shipped: grammar (`parse_type`), runtime
  (`type-matches?` passes unknown names), and static unification
  (`SigWithVars`/`SigTerm` in `ctx.rs`; `parse_sig_decl_with_vars` in
  `annot.rs`; `expr_ty` resolves return types per-call from arg types).
  See [`docs/type-variables.md`](type-variables.md).

## How it runs — and why it's outside the runtime

The checker is a **pre-step at the file/project boundary**, never woven into
evaluation:

- `brood --check <file>…` — check one or more files (the language binary).
- `nest check [FILE…]` — check the whole project, or specific files (the CI /
  editor entry point). Both forms run one Brood path that loads the project image
  first, so a single-file check resolves cross-namespace `:use`/qualified names
  exactly as a whole-project check does.
- `brood <file>` — check, then run a file.
- `nest run` — check, then run: the `:main` entry path checks the project sources
  (`check-project-sources` in `run-project`), and `nest run FILE.blsp` (an explicit
  file) pre-checks that file (`check-file`, a single-file check like `brood <file>`).
  So every run path checks first; `BROOD_NO_CHECK=1` opts out.
- `nest test` — check the project, then run the tests.
- **Not** in the REPL / `load` / per-form `eval` (maybe later) — so there's no
  per-eval noise and no suppression machinery beyond the `try`/`error-of` skip.

**Checking is upstream of hot reload, never part of it.** "Don't reload code we
can already see will fail" is a property of the *workflow that orchestrates the
reload* — today: run `brood --check` first; later: the editor's reload command
(itself Brood) checks, then reloads — **not** of the `def`/reload primitive. The
runtime never consults the checker, so: contract #5 holds with **no carve-out**,
there is nothing to override, and a wrong signature can at worst print a stray
warning upstream — it can never wedge a reload. (Reloads should be *atomic* —
broken new code leaves the running version in place — but that's hot-reload
hygiene, independent of the type checker.) The type system stays **entirely
external**: it observes and advises; it is never in the execution or reload path.

Why no inference *engine* (ADR-011): full body inference needs control-flow /
dominance analysis to avoid false positives from *guarded* uses (a param used as
a number only inside `(if (number? x) …)` doesn't make the param a number). That
machinery is the bulk of the complexity and the only false-positive source — so
we cut it, keeping curated sigs + the trivially-sound straight-line case, and add
more only when a concrete need justifies it.

## Compatibility contract

Every change — new primitive, new special form, new `Value` kind, new feature —
must keep these true, so future work stays on the set-theoretic path. Items
marked **(enforced)** are compile errors if violated; the rest are review rules.

1. **Every value has exactly one tag.** The `Tag`s are the type atoms, and a
   tag's `#[repr(u8)]` discriminant *is* its lattice bit. A new `Value` variant
   must get a `Tag` (in `value::tag`, **enforced** — exhaustive match) and be
   added to `types::ALL_TAGS`; `TAG_COUNT`/`UNIVERSE` then follow automatically.
   The `tag_universe_is_consistent` test checks bits are dense and in order, so a
   tag *missing from* or *misordered in* `ALL_TAGS` fails CI (the gap a plain
   match can't catch, since Rust can't enumerate variants). Don't introduce a
   value kind that can't be a tag. **There are 17 tags today** (…`Rope`, `Socket`),
   and the lattice's tag bitset is a **`u32`** (`Ty { tags: u32, … }`, ADR-078), so
   it has headroom to 32 atoms. `UNIVERSE` computes in `u64` and narrows to dodge
   the `1u32 << 32` const-overflow at the cap; a *33rd* tag must widen the `tags`
   field to `u64` (the `TAG_COUNT <= 32` assert in `types/mod.rs` is the tripwire).
2. **A type is a set of values.** Don't add a typing concept that isn't a set
   (no nominal-only identity, no escape hatch that breaks set semantics).
   Structured types arrive as proper set-theoretic extensions, never bolt-ons.
3. **Subtyping is inclusion.** Never add an ad-hoc subtyping rule. `a <: b` iff
   `a`'s value set ⊆ `b`'s — full stop. This is precisely what keeps us off the
   TypeScript route.
4. **Redefinable bindings are `dynamic()`, never assumed static.** Any feature
   touching `def` / globals / hot reload must keep them `dynamic()` so a checker
   can never contradict a future redefinition. This is the "don't inhibit the
   language" invariant.
5. **Checking is advisory.** No change may let a type result *reject* a runnable
   program — except provably-sound special-form *structure* errors (special forms
   aren't redefinable, so those can't be wrong). Types warn and optimise; they
   never gate.
6. **Every primitive declares its type. (enforced)** A new builtin supplies a
   result `Ty` (+ arg `Ty`s) next to its `Arity` — `NativeFn` carries a `Sig`
   field, the same mechanism that makes `Arity` mandatory: omitting it is a
   compile error. The "no useful info" case uses `Sig::any()` (overlaps every
   input, never warns), so the contract holds for permissive builtins too.
7. **Policy in Brood.** If a type test or contract can be written in Brood over
   `type-of`/predicates, it goes in `std/`, not Rust (ADR-006).
8. **Pattern/guard forms expose their refinement.** New pattern kinds or guards
   must remain analysable for occurrence typing — the matcher is the inference
   goldmine (step 4). Don't add opaque guards that hide the type they imply.
9. **Errors, `type-of`, and `Ty` agree on names.** All use `Tag::name`
   spellings, so a `Ty` in a message reads the same as `type-of` returns.

## Where it lives

(After the `core/` / `syntax/` / `eval/` / `types/` module split.)

- `crates/lisp/src/types/mod.rs` — the `Ty` lattice (step 1), `GradualTy`
  (step 2), and `tested_by` (the guard-narrowing bridge for step 4).
- `crates/lisp/src/types/check.rs` — the checker's entry points (`check_form`,
  `check_file`) + the in-source test suite; the work is split across the
  `check/` submodules:
  - `check/walk.rs` — the recursive `check_into`, the per-special-form helpers,
    `SPECIAL_HEAD` dispatch, the arity / unbound / callback-arity checks.
  - `check/sigs.rs` — signature + arity sources (primitive / curated / inferred).
  - `check/guards.rs` — guard recognition, narrowing, `expr_ty`, sequence-aware
    result types.
  - `check/ctx.rs` — the `Ctx` threaded through the walk (narrowings, aliases,
    file-globals, `SigWithVars` type-variable unification).
  - `check/annot.rs` — `(sig …)` declaration parsing (un-expanded tree).
  - `check/protocol.rs` — protocol / behaviour conformance.
  - `check/recursion.rs` — the non-tail self-recursion lint.
  - `check/hygiene.rs` — the macro-hygiene capture lint.
- `crates/lisp/src/core/value.rs` — `Tag` (the atoms), `value::tag`, `NativeFn`
  (carries the `Sig` the checker reads — contract point #6).
- `crates/lisp/src/eval/mod.rs` — `call_native` (the arity gate).
- `crates/lisp/src/eval/macros.rs` — `macroexpand_all`, the pass the checker runs
  after.
