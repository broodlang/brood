# Type annotations — `(sig …)` and the road to sound gradual typing

**Status:** slices 1–10 shipped — slice 1 (`(sig …)`, checker-facing), slice 2
(`(sig! …)`, runtime enforcement), slice 3 (`BROOD_CONTRACTS=1`), slice 4
(element-level `(list E)`/`(vector E)` checks), slice 5 (`&` rest params),
slice 6 (`(and A B …)` intersections — runtime + checker), slice 7
(`(map K V)` key/value contracts — runtime), slice 8 (`?A` type variables —
grammar + runtime passthrough), slice 9 (`(map K V)` full checker refinement
— `Ty::map_of`, `get`/`keys`/`vals`/`assoc` result rules), slice 10
(`?A` `SigTerm`/`SigWithVars` unification — per-call return-type resolution),
and the **gradual checks** — slice 11 (`(def x …)` vs a non-arrow `(sig x T)`
value type, `GradualTy`'s first consumer), slice 12 (return-type checking +
declared globals in value position), slice 13 (precise sig-param returns — the
first non-disjoint "merely wider" catch). See [the gradual checks](#the-gradual-checks-slices-1113)
and [ADR-110](decisions.md).

This is Brood's answer to "can we be *more sound* given our parameters?"
(advisory, never-gate, zero-false-positive, hot-reload, policy-in-Brood). The
mechanism is the Elixir paper's **strong arrow**, done Brood's way: a function
that **checks its arguments at run time** can be *trusted* by the static checker.
We get soundness by leaning on a runtime check — not by inserting casts and not
by gating compilation. See [`research/set-theoretic-types-in-brood.md`](research/set-theoretic-types-in-brood.md)
and [`types.md`](types.md).

## Surface

A top-level declaration form:

```clojure
(sig area  (number -> number))
(sig clamp (number number number -> number))
(sig const (any -> any))
```

`sig` (not `::` — a leading `:` lexes as a keyword in Brood, so `(:: …)` is a
keyword-headed list, unusable as a form head). The arrow marker `->` reads as an
ordinary symbol, so `(number -> number)` is a plain list the parser splits on
`->`.

### Type-expression grammar (slice 1)

```
type   ::= base | literal | typevar | arrow | seq | map-kv | union | inter
base   ::= any | never | int | float | number | string | symbol
         | keyword | bool | nil | pair | vector | list | map | fn
         | rope | pid | ref | socket
literal ::= <keyword>                          ; a bare keyword, e.g. :maximized
typevar ::= ? <name>                           ; e.g. ?A, ?el — static only
arrow  ::= ( type* -> type )                   ; fixed arity
         | ( type* & type -> type )            ; fixed leading params + variadic rest
         | ( type* &optional type* -> type )   ; fixed params + optional (ADR-127)
         | ( type* &optional type* & type -> type ) ; + a trailing rest too
seq    ::= (list type) | (vector type)         ; element type checked at runtime
map-kv ::= (map key-type val-type)             ; key/val checked at runtime
union  ::= (or type type+)
inter  ::= (and type+)                         ; intersection; (and) = any
compl  ::= (not type)                          ; complement — every value that is NOT a type
```

**The complement `(not T)` (ADR-263).** Every value that is *not* a `T`. The
lattice has computed complements since ADR-023 — the else-branch of a `(string?
x)` guard is one — but there was no way to write one, so "anything but nil", the
most-wanted annotation in a nil-carrying language, could not be said:

```lisp
(sig head-of ((and any (not nil)) -> any))
```

Exact on the flat tag lattice; the complement of a *refined* type (`(not (tuple
int))`) widens to that tag, which over-approximates — sound, and it can only ever
suppress a warning. Exactly one argument: `(not A B)` is reported as malformed
rather than guessed. The runtime `type-matches?` implements the same clause, so a
`sig!` contract agrees with the checker.

**Keyword-literal (singleton) types (ADR-105).** A *bare* keyword in type position
is a literal type — the value must be exactly that keyword. Enumerate a closed set
with `(or …)`: `(or :maximized :fullboth :fullscreen nil)`. Write keywords bare, not
`'`-quoted — they're self-evaluating and unambiguous in type position, and bare is
what the runtime `(sig! …)` contract matches by equality. A keyword outside the set
is flagged by the checker/LSP and throws under a runtime contract; the diagnostic
names the exact value (`got :bogus`).

**Int/bool/string-literal (singleton) types (ADR-117/119).** The same
machinery, three more kinds — `(or 200 404 500)`, `(or true false)`,
`(or "GET" "POST")` — same runtime-contract behavior as the keyword case, and
any combination composes freely on one declared type (`(or :ok 5)`). `false`
**is** a legitimate literal type now — the earlier "`false` is not a literal
type, use `nil`" guidance was scoped to the keyword-only era (avoiding
`false`/`nil` confusion in an *enumerated keyword* set specifically), not a
technical restriction; now that bool-literal types are their own real kind,
`(sig f (false -> any))` means exactly what it looks like. Call-site literal-argument precision **shipped as Gap B0** (2026-07-10): a
literal int/bool/string *argument* is now recognized as a singleton via
`Ty::of_value`/`expr_ty`, the way a literal keyword argument always was (an
early int attempt was reverted; B0 landed it cleanly). See
[type-int-literals.md](type-int-literals.md)/[type-bool-string-literals.md](type-bool-string-literals.md)
for the history.

**Match exhaustiveness and redundancy (ADR-118/120/121).** A `match` over a
scrutinee whose declared type is a pure enumerable literal type (any mix of
the kinds above, plus `nil`) is flagged when its clauses don't cover every
member (unless a catch-all is present); a clause whose literal duplicates one
already tried is flagged as unreachable, whether or not it came from `match`.
See [type-match-exhaustiveness.md](type-match-exhaustiveness.md) and
[type-match-redundancy.md](type-match-redundancy.md).

**Dead-clause lint (ADR-131).** A guard (`cond` predicate or `match` literal
pattern) that narrows a *typed* binding to the empty type flags the branch as
dead code — `is X, which can never be Y`. It fires for two kinds of binding: a
**sig-typed parameter** (`(sig f (int -> …))` + `(cond (string? n) …)`), and a
**precise surface `let`-local** (`(let (port 8080) (cond (string? port) …))`).
Eligibility is deliberately narrow — a `let`-local qualifies only when its RHS is
**precise** (a literal / integer-closed expression, so `gradual_of.dynamic ==
false`; a call-result or redefinable-global binding is `dynamic` and excluded,
keeping the verdict reload-safe) and its name is **surface** (not a gensym macro
temp). That binding-level gate is the whole of the surface-vs-generated scoping:
a macro tests its own gensym temps, never the user's named local, so no guard-site
position check is needed. Sound because a local is immutable within its scope, so
an over-approximated-but-precise type narrowed to `never` genuinely proves the
branch dead. A negative test that *deliberately* writes a dead clause opts out
with `(check-allow :unreachable-clause …)`.

**`:unbound` (ADR-145).** `(check-allow :unbound …)` suppresses the
unbound-symbol lints over the wrapped forms — for globals defined at
*runtime* that the source checker cannot see: an `eval`-driven `def`, the
wasm `use-native` binding (which defines a Brood fn per component export at
load time). The one lint whose ground truth is the live image rather than
the source; everywhere else, prefer fixing the reference.

**`&optional` params (ADR-127).** `(sig f (int &optional string -> int))`
declares `f`'s second argument as optional, mirroring a closure's own
`(a &optional b)` shape; combine with a trailing `& rest` for all three
kinds together (`(int &optional string & number -> int)`), matching a
closure's full `(req &optional opt & rest)` param list. Both the call-site
argument-type check and arity checking treat the declared range correctly
(omitting an optional arg is fine; supplying it is type-checked; one
argument beyond required+optional is still an arity error). Inside the
body, an optional param is seeded as `T | nil` rather than the exact `T` a
required param gets — it may genuinely be absent — so a defensive
`(nil? b)` check is never mistaken for dead code, while using it
unconditionally as if it can't be `nil` is still caught. `&optional` before
`&` in a sig is required (mirroring reader order); the reverse order is
dropped by the parser rather than misparsed.

Base names map to the same lattice points the predicates imply (`number` =
`int∪float`, `list` = `nil∪pair`, `fn` = `fn∪native`, `seqable` =
`nil∪pair∪vector∪set∪map∪bytes` — every collection the sequence combinators walk, `string`
excluded — for a polymorphic-sequence parameter without falling back to `any`, …).

### A declaration that cannot be read is reported, not dropped (ADR-259)

A `sig` is read *first* — ahead of the primitive table, the curated table and
inference — so a declaration the parser cannot read used to be worse than none:
the annotated position silently widened to `any`. Four shapes are now reported:

```lisp
(sig q1 (strng -> int))          ; unknown type `strng`
(sig q2 ((tupel int) -> int))    ; unknown type constructor `tupel`
(sig q3 (int -> int))            ; …beside (defn q3 (a b) …): arities disagree
(sig q4 (int -> int))            ; nothing named `q4` is defined here
```

The third used to *suppress* a correct check: within a file the declared sig was
the only arity source, so a wrong one made a wrong call type-check clean and die
at run time. The **definition** now owns the arity, and only a provably-disjoint
declaration is reported (a multi-arm `defn` annotated with one arm's arrow
overlaps, and stays silent).

One deliberate silence: an unknown **capitalised** name is assumed to be an
ability used as a type (ADR-181/186), whose module a single-file check may not
have loaded.

A `(sig name (… -> …))` whose type-expr is an **arrow** declares a function
signature. Non-arrow `(sig x int)` (a value's type) declares a **value type**:
it's consumed by the **gradual-assignment check** (`GradualTy`'s first consumer),
which verifies a `(def x <expr>)` assigns a value *consistent* with the declared
type — flagging `(def x "s")` against `(sig x int)`, and `(def x g)` when `g`'s
own declared type is disjoint from `x`'s, while deferring on a dynamic value
(an over-approximated call, an unknown global) so hot reload is never fought.

## How the checker uses it (slice 1 — shipped)

- `check_file` scans the **un-expanded** top-level forms for `(sig name …)`
  (the `sig` macro expands to `nil`, so this must run before expansion — same as
  the hygiene lint), parses each to a `Sig`, and stores `name → Sig` on the
  `Ctx` (`Ctx.declared`).
- A declared sig is consulted **first** — ahead of primitive / curated / inferred
  sigs — in the call-check path (`walk`) and in `expr_ty` (for the result type).
  So `(foo "x")` is flagged against the declared params, and
  `(string-length (foo 3))` against the declared result.
- Arity falls back to the declared param count when the callee isn't otherwise
  resolvable (a file-local `defn` the read-only checker can't inspect).

This already closes the **biggest expressiveness gap**: multi-clause / branchy
user functions, which `infer_sig` can't touch, now participate in checking the
moment the author writes one line of `sig`.

**Slice 1 is not yet *sound*.** Nothing forces `foo` to actually obey
`(int -> int)` — the checker simply *trusts* the declaration (TypeScript-style).
A lying annotation can still let a wrong value through. That's the job of slice 2.

## Slice 2 — runtime enforcement via `(sig! …)` (shipped)

`(sig! name (P… -> R))` declares the signature *and* installs a **runtime
contract**: it rebinds `name` to a same-arity wrapper that checks each argument
against `P…` and the result against `R`, **throwing** on a mismatch. That makes
`name` a *strong arrow* — applied off-domain it returns a value in `R`, fails a
runtime check, or diverges; it can never silently return an off-type value. The
checker reads `(sig! …)` exactly like `(sig …)`, so the static trust is now
**sound** — the reported type holds unless the program throws (the paper's
(i)/(ii)/(iii) guarantee).

It's **all policy in Brood** (no new primitive): the `sig!` macro generates the
wrapper, `type-matches?` decides membership over `type-of`/predicates, and
`contract--check-args` does the per-argument check (all in `std/prelude.blsp`).
Place `(sig! …)` **after** the definition (it rebinds the name). The wrapper
preserves arity, so introspection and the reload-arity diagnostic are
undisturbed (the one cost: `arglist` of a wrapped fn reflects the wrapper).

Design decisions, as built:
- **Where the check lives** — the wrapper rebinds the **global**, so every call
  is checked, including indirect / `apply`.
- **Opt-in** — only `(sig! …)` enforces; plain `(sig …)` stays static-only and
  free. Writing a *type* never changes behaviour; opting into *enforcement* does.
- **Unknown types accept** — a type-expr `type-matches?` can't interpret (an
  unknown base name, an arrow param) accepts any value, so a contract never
  throws on a type it doesn't understand (no spurious runtime failure).
- **Hot reload** — re-`def`ing `name` drops the contract (it's the binding);
  re-run `(sig! …)` to reinstall. The wrapper's preserved arity keeps the
  reload-arity check quiet.

Verified by `tests/contract_test.blsp`: a correct call passes; a bad argument,
a bad *result* (a fn that lies about its return type), and a union-type
non-member all throw.

**Also shipped (slices 3–8):** `BROOD_CONTRACTS=1` enforces every `(sig …)` as
a runtime contract (same as `sig!`) for a dev/test run; element-level checks
walk `(list E)` / `(vector E)` arguments at call time; `&` rest params let
`(sig! f (int & number -> int))` check both fixed and variadic arguments;
`(and A B …)` intersections are enforced at runtime and parsed by the static
checker (`Ty::intersect`); `(map K V)` checks every key/value pair at runtime
and the checker flat-accepts the annotation as `Ty::Map`; and `?A` type
variables are parsed by both runtime and checker (resolved to `any` / `Ty::ANY`
— the static-only constraint is not yet unified at call sites) (superseded by
slice 10 — call-site unification shipped). See
`tests/contract_test.blsp` for coverage.

## The gradual checks (slices 11–13)

These are the first consumers of `GradualTy` (`crates/lisp/src/types/mod.rs`) — the
*set-theoretic* gradual type `dynamic()` (ADR-024). The key realisation
([ADR-110](decisions.md)): the existing **disjointness** pass over `Option<Ty>` gets
gradual behaviour for free (an unknown is silent = `dynamic()`), so `GradualTy` adds
nothing *there*. It earns its place only in a check with **assignment / subtyping**
semantics — one that errors when a value is *not a subtype* of where it flows — because
that's where consistency (and the gradual benefit-of-the-doubt) actually differs from
disjointness. `walk::gradual_of` maps an expression to a `GradualTy`; `consistent_with`
decides the check.

- **Slice 11 — assignment.** A non-arrow `(sig x T)` declares `x`'s *value type*;
  `(def x <expr>)` is checked to assign a value consistent with `T`. `(def n "hi")`
  against `(sig n int)` flags.
- **Slice 12 — return types + value-position globals.** A `(sig f (P… -> R))` checks the
  body's last form yields a value consistent with `R` (`(sig f (int -> string))` with body
  `(+ x 1)` flags — `number ∩ string = ⊥`). And `expr_ty` now surfaces a declared global's
  type, so `(string-length g)` with `(sig g int)` is caught.
- **Slice 13 — precise sig-param returns.** A `(sig …)`-typed parameter carries its *exact*
  contract type, so returning one where it's *merely wider* than `R` is caught:
  `(sig f (number -> int)) (defn f (x) x)` flags. This is the first diagnostic the
  disjointness checker structurally can't produce.

**The capability `Option<Ty>` can't express:** a redefinable global with a declared type is
`dynamic_within(t)` — a *bounded dynamic* (`Option<Ty>` has only known/unknown). So
`(def count label)` with `label : string`, `count : int` is flagged (`string ∩ int = ⊥`),
which the disjointness pass — every global an untracked `None` — misses. This is exactly
the hot-reload motivation of ADR-024: warn on a provable mismatch with the declared
*contract*, defer when the bound merely overlaps.

**The false-positive rule (why none of this regressed the zero-FP bar):** an
over-approximated value (a call result — `(+ int int)` is typed `number`) is
`dynamic_within(t)`, so consistency uses `∩ ≠ ⊥` and never over-warns on a widened guess
(`(def n (+ 1 2))` vs `int` *defers*); only a **precise** value — a literal or a sig-param —
is `stat(t)` and checked with `⊆`. Project-wide `nest check` stayed at 3 (the intentional
recursion lint) through all three slices.

**Deferred (ADR-011):** catching a wider *call-result* body (typed `number`, declared `int`)
needs precise result types — overloaded arithmetic sigs (`(+ int int) : int`) or full
occurrence-typing inference (the historical false-positive source). Gated on a real
consumer; the overloaded-sig option is the bounded next step.

## Fixed gap — a `defmodule`-declared arrow sig didn't seed the body-return check (ADR-126)

Discovered 2026-07-05 while building ADR-125's end-to-end `--watch` smoke
test, unrelated to that feature; fixed the same day. `(sig fname (A -> B))`
followed by `(defn fname …)` **inside a `defmodule` block** didn't seed
`check_def`'s body-vs-declared-return-type check, even though the identical
pattern at the root namespace (no `defmodule`) worked correctly. Repro:

```
(defmodule m "doc")
(sig f (-> string))
(defn f () 42)   ; now warns "f: declared return type string ... yields int"
```

**Root cause:** Pass 2.5 (`annot::parse_sig_decl`) scans un-expanded forms and
records the declared sig under the symbol **as literally written** (bare
`f`), with no `resolve_reference`/qualification step. But `defn f` inside a
`defmodule` expands to `(def m/f (fn …))` — the qualified symbol. `check_def`'s
seeding lookup (`ctx.declared_sig(name)`, `walk.rs`) used `name` from the
*expanded* form (`m/f`), which never matched the bare-keyed `f` Pass 2.5
recorded — so the sig was invisible to the seeding path. Call-site checking
(`sig_of`) never had this problem: it falls back through
`declared_heap_sig`, which reads the heap-wide store `%register-sig`
populates *with* qualification (requires the file to have actually been
`eval`'d — true for `nest check`'s whole-project mode, which loads sources
first). `check_def`'s body-return seeding had no such fallback.

**Fix:** gave `check_def`'s seeding lookup the same `declared_heap_sig`
heap-wide fallback ADR-124 gave the value-sig path:
`ctx.declared_sig(name).or_else(|| declared_heap_sig(heap, name))`. Verified
with the revert-then-confirm technique used throughout this session's
checker changes (`defmodule_declared_arrow_sig_seeds_return_type_check`
fails with the fix reverted, passes restored). `nest check`'s whole corpus
(`std/` + `tests/`) stayed at 91 warnings before and after — the pattern
this fixes (a genuinely mismatched `defmodule`-qualified `sig`+`defn` pair)
doesn't currently occur anywhere in the committed source, so the fix closes
the gap without surfacing any pre-existing bugs. See ADR-126.

## Suppressing an advisory lint on purpose — `(check-allow :category form…)`

The advisory lints are false-positive-clean, but some test code *deliberately*
trips a correct lint: a non-tail-recursive function written to exercise the
deep-recursion / JIT-recursive-arm path, or a redundant `match` clause proving the
first match wins. The lint is right; the warning is unwanted. Comments can't
express the opt-out — the reader strips them before the checker runs — so the
directive is a form:

```lisp
(check-allow :non-tail-recursion
  (defn ut-fib (n) (if (< n 2) n (+ (ut-fib (- n 1)) (ut-fib (- n 2))))))

(assert= (check-allow :unreachable-clause (match 1 (1 :first) (1 :second) (_ :z)))
         :first)

;; A negative test that deliberately violates its own `sig` — proving the `sig!`
;; runtime contract throws — opts the static return/argument lint out.
(check-allow :type-mismatch
  (defn c-bad-ret (x) "not an int"))
(sig! c-bad-ret (int -> int))

`check-allow` is a prelude macro that expands to a `(%lint-allow :category (do …))`
marker. That marker **survives macroexpansion** (which is what the checker walks)
and is a **pure runtime no-op** — `%lint-allow` just yields its body's value, so a
wrapped top-level `defn` still defines globally and a wrapped expression still
returns its value. It wraps one form or many.

The checker reads the marker: `recursion.rs` skips a `:non-tail-recursion`-tagged
subtree entirely, and a `SUPPRESS_*` bit (see `check/ctx.rs`) threads down the walk
so `check_if`'s redundant-clause lint declines inside an `:unreachable-clause`
scope. Recognised categories today: **`:non-tail-recursion`**,
**`:unreachable-clause`**, **`:type-mismatch`** (a `sig`-declared return or
call-site argument the wrapped code deliberately violates), **`:unbound`** (a name
bound only at runtime), and **`:unrequired`** (ADR-189 — a qualified `mod/name` whose
module the file deliberately reaches via another require, e.g. a circular dependency
that can't `(require 'mod)` at the top level). An unrecognised
category suppresses nothing — a typo is a
no-op that still lints, never a silent blanket opt-out. This is what lets
`nest check` stay at **zero** warnings project-wide without weakening any lint.

## Why this is the right "more sound" move for Brood

Classic type soundness needs gating; we don't gate. Sound *gradual* typing
classically needs inserted casts; we don't change compilation. The strong-arrow
route gives soundness from a runtime check the programmer opted into — advisory
when you don't annotate, sound exactly where you do, and never in the way of hot
reload. It also hands the editor real declared types for hover/completion — the
consumer that actually justifies the work.
