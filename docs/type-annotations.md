# Type annotations — `(sig …)` and the road to sound gradual typing

**Status:** slices 1–10 shipped — slice 1 (`(sig …)`, checker-facing), slice 2
(`(sig! …)`, runtime enforcement), slice 3 (`BROOD_CONTRACTS=1`), slice 4
(element-level `(list E)`/`(vector E)` checks), slice 5 (`&` rest params),
slice 6 (`(and A B …)` intersections — runtime + checker), slice 7
(`(map K V)` key/value contracts — runtime), slice 8 (`?A` type variables —
grammar + runtime passthrough), slice 9 (`(map K V)` full checker refinement
— `Ty::map_of`, `get`/`keys`/`vals`/`assoc` result rules), slice 10
(`?A` `SigTerm`/`SigWithVars` unification — per-call return-type resolution).

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
seq    ::= (list type) | (vector type)         ; element type checked at runtime
map-kv ::= (map key-type val-type)             ; key/val checked at runtime
union  ::= (or type type+)
inter  ::= (and type+)                         ; intersection; (and) = any
```

**Keyword-literal (singleton) types (ADR-105).** A *bare* keyword in type position
is a literal type — the value must be exactly that keyword. Enumerate a closed set
with `(or …)`: `(or :maximized :fullboth :fullscreen nil)`. Write keywords bare, not
`'`-quoted — they're self-evaluating and unambiguous in type position, and bare is
what the runtime `(sig! …)` contract matches by equality. A keyword outside the set
is flagged by the checker/LSP and throws under a runtime contract; the diagnostic
names the exact value (`got :bogus`). `false` is *not* a literal type — use `nil`
for an "off" arm. (Bool/int/string literals are the same machinery, deferred.)

Base names map to the same lattice points the predicates imply (`number` =
`int∪float`, `list` = `nil∪pair`, `fn` = `fn∪native`, …). Deferred: map K/V
full checker refinement (`map_kv` in `Ty`); type variable unification at call
sites (`SigTerm` route — see [type-variables.md](type-variables.md)).

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
— the static-only constraint is not yet unified at call sites). See
`tests/contract_test.blsp` for coverage.

## Why this is the right "more sound" move for Brood

Classic type soundness needs gating; we don't gate. Sound *gradual* typing
classically needs inserted casts; we don't change compilation. The strong-arrow
route gives soundness from a runtime check the programmer opted into — advisory
when you don't annotate, sound exactly where you do, and never in the way of hot
reload. It also hands the editor real declared types for hover/completion — the
consumer that actually justifies the work.
