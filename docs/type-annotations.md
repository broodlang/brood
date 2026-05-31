# Type annotations — `(sig …)` and the road to sound gradual typing

**Status:** design + slice 1 (checker-facing) landing. Slice 2 (runtime
enforcement) is the soundness step and is specced here but not yet built.

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
type   ::= base | arrow | seq | union
base   ::= any | never | int | float | number | string | symbol
         | keyword | bool | nil | pair | vector | list | map | fn
         | rope | pid | ref | socket
arrow  ::= ( type* -> type )          ; params before ->, one result after
seq    ::= (list type) | (vector type)
union  ::= (or type type+)
```

Base names map to the same lattice points the predicates imply (`number` =
`int∪float`, `list` = `nil∪pair`, `fn` = `fn∪native`, …). Deferred (ADR-011, add
on demand): intersections, rest/optional params, map key/value types, type
variables.

A `(sig name (… -> …))` whose type-expr is an **arrow** declares a function
signature. Non-arrow `(sig x int)` (a value's type) is accepted by the grammar
but ignored by the call-checker in slice 1 (nothing consumes it yet).

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

## Slice 2 — runtime enforcement (the strong arrow; not yet built)

`(sig name (P… -> R))` will also install a **runtime contract**: calls to `name`
check each argument against `P…` and the result against `R`, **throwing** on a
mismatch. That makes `name` a *strong arrow* — applied off-domain it returns a
value in `R`, or fails a runtime check, or diverges; it can never silently return
an off-type value. The checker's trust then becomes **sound**: the static type it
reports genuinely holds unless the program errors at run time — the paper's
(i)/(ii)/(iii) guarantee.

Design questions to settle for slice 2:
- **Where the check lives** — wrap the bound global closure (so every call is
  checked, including indirect/`apply`), vs a call-site check (cheaper, misses
  indirect calls). Wrapping the global is the sound choice.
- **Cost / opt-out** — a `BROOD_NO_CONTRACTS=1` (or per-sig opt-out) for hot
  paths, mirroring `BROOD_NO_CHECK`. Contracts are policy in Brood, so this can
  be a Brood-level switch.
- **Hot reload** — re-`def`ing `name` must re-install (or drop) its contract;
  the contract is metadata on the binding, not the closure value.
- **Result-type checks** and higher-order params (does a `(int -> int)` *param*
  get wrapped so the callback is checked?) — defer the HOF case.

Soundness oracle to add with slice 2: a `(sig)`-declared function whose body
returns the wrong type must **throw at runtime**, never corrupt — the runtime
backstop the static trust relies on.

## Why this is the right "more sound" move for Brood

Classic type soundness needs gating; we don't gate. Sound *gradual* typing
classically needs inserted casts; we don't change compilation. The strong-arrow
route gives soundness from a runtime check the programmer opted into — advisory
when you don't annotate, sound exactly where you do, and never in the way of hot
reload. It also hands the editor real declared types for hover/completion — the
consumer that actually justifies the work.
