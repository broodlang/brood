# Type variables — parametric signatures for user-defined generic functions

> Status: **design only**. Not yet started. This is Option A from
> `parametric-result-types.md` — deferred until a real consumer exists. The
> HOF cases (`map`/`filter`/`reduce`/`fold`) were already handled via per-HOF
> result rules (Option B) and don't need this.

## Problem

User-defined generic functions have no way to express a "same type in, same type
out" relationship. Today every user generic must either use `any` (losing type
flow) or duplicate concrete signatures:

```lisp
;; I want to say: returns an element of the same type the list contains
(sig my-first ((list ?A) -> ?A))

;; I want to say: identity — returns whatever it receives
(sig identity (?A -> ?A))

;; I want to say: applies f to each, same container element type
(sig my-map ((?A -> ?B) (list ?A) -> (list ?B)))
```

Without this, downstream checkers lose element types at user HOF boundaries —
they propagate through `map` (curated) but die at `my-map` (uncurated). As
user codebases grow and people write their own HOF utilities, this becomes a
real gap.

## Syntax

Type variables are **symbols starting with `?`** — unambiguous, never a valid
base-type name, and composable with all existing type expressions:

```
type-var ::= ? <symbol>              ; e.g. ?A, ?el, ?key
type     ::= … | type-var
           | (list type-var)         ; element type as a var
           | (type-var -> type-var)  ; arrow with vars
```

Variables are scoped to **one `sig`/`sig!` declaration**. Two uses of `?A` in the
same declaration refer to the same type; `?A` in one `sig` and `?A` in another
are independent.

## Checker — unification at the call site

When the checker encounters a call `(f arg1 arg2)` and `f`'s declared sig
contains type variables, it:

1. **Builds a substitution** `σ : var → Ty` by unifying each arg's known type
   against the corresponding param type in the sig. Unknown arg types (vars,
   complex expressions) are skipped — contributing no constraint.
2. **Applies σ to the result type.** Unresolved vars in the result widen to
   `Ty::ANY`.
3. **Checks each arg against its param type** *after* substituting σ — an
   arg that is disjoint from its substituted param type fires a warning.

This is one-level shallow unification — no recursive types, no higher-kinded
vars. It handles the practical cases (`identity`, `my-first`, `my-map`) without
a full Hindley-Milner fixpoint.

### Unification rules (one-way, left-to-right: param ← arg)

| Param | Arg known type | Binds |
|-------|---------------|-------|
| `?A`  | `T` (any)     | `?A → T` |
| `(list ?A)` | `nil\|pair` (bare list) | nothing — unknown element |
| `(list ?A)` | `list<T>` | `?A → T` |
| `(vector ?A)` | `vector<T>` | `?A → T` |
| `(?A -> ?B)` | arrow `S → R` | `?A → S`, `?B → R` |
| `(or ?A int)` | `T` | `?A → T` (widen the union; skip — see below) |

Union/intersection params containing type vars are treated as **opaque** (no
binding extracted) — a union `(or ?A int)` can't pin `?A` from a single concrete
arg. Conservative: miss the refinement rather than risk a wrong binding.

When two args both bind the same var (`?A` appears in params 1 and 3), the
bindings are **unioned** — `?A → T1 ∪ T2`. This is sound (we over-approximate)
and matches how element types in `reduce` already work.

### Runtime — type variables in `sig!`

At runtime, a type variable is an *unknown* type. The `type-matches?` function
(which drives `sig!` checks) already returns `true` for unknown type names:

```lisp
;; in the (symbol? t) / else branch of type-matches?:
else true   ; unknown base name → accept
```

A type-var symbol like `'?A` is unknown to `type-matches?`, so **`sig!` with
type variables performs no runtime check on variable positions** — identical
to the existing "unknown type → accept" rule. No change to `type-matches?`.

This is correct: a type variable is a static constraint, not a runtime one. The
runtime contract is still useful for the non-variable parts of the signature —
`(sig! my-map ((?A -> ?B) (list ?A) -> (list ?B)))` still checks that arg 1 is
a fn and arg 2 is a list.

## Representation

### Brood side — none

Type variables are valid Brood symbols starting with `?`. `parse_type` on a
`?A` symbol currently returns `None` (unknown name → drop annotation). After the
change it will return `Ty::ANY` (accept everything — the checker substitution
handles the precision). No change to `type-matches?` or `sig!`.

### Rust side (`annot.rs` + `types/mod.rs` + `walk.rs`)

**Option A: `Ty::Var(u32)` lattice change.**

Add a new `Ty` variant that carries a variable index. The set-theoretic operators
must handle it — `union(Var(i), Var(j))` is unanswerable without a substitution,
so every op that touches a `Var` either resolves it via σ or widens to `Ty::ANY`.
The lattice grows a new kind that breaks the "a type is a bitset of tags" invariant
(compatibility contract point #2), requiring significant review.

**Option B: variables only at the `Sig` level (RECOMMENDED).**

`Ty` stays a pure bitset-plus-refinements. Type variables live in a new
`SigTerm` enum used only in `Sig` fields — not in `Ty` itself:

```rust
// crates/lisp/src/types/mod.rs  (or check/annot.rs)
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SigTerm {
    Ty(Ty),
    Var(u32),             // ?A, ?B … assigned sequential indices at parse time
    ListOf(Box<SigTerm>),
    VectorOf(Box<SigTerm>),
    Arrow(Vec<SigTerm>, Box<SigTerm>),
}
```

`Sig` changes from `{ params: Vec<Ty>, rest: Option<Ty>, ret: Ty }` to
`{ params: Vec<SigTerm>, rest: Option<SigTerm>, ret: SigTerm }`.

The checker resolves `SigTerm → Ty` at each call site by:
1. Walking the param list, building `HashMap<u32, Ty>` by unification.
2. `resolve(term, σ)` → Ty: `SigTerm::Ty(t)` → `t`; `SigTerm::Var(i)` →
   `σ.get(i).cloned().unwrap_or(Ty::ANY)`.

**Why Option B honours the compatibility contract:**
- `Ty` itself is unchanged — every existing consumer (disjointness, subtyping,
  guards, primitives) is untouched.
- `Sig` grows from `Vec<Ty>` params to `Vec<SigTerm>` params — a breaking change
  to `Sig`'s type, but `Sig` is already private to the checker and not part of
  the public `brood` API (only used in `NativeFn.sig` and `check/`).
- Primitive sigs (on `NativeFn`) use `SigTerm::Ty(…)` for all fields — the
  existing construction is still valid, just wrapped in the enum variant.

**Blast radius of the `Sig` change:**
- `NativeFn::sig` field type changes — every primitive's `Sig::new(…)` call
  needs wrapping: `Sig::new(vec![SigTerm::Ty(t1), …], SigTerm::Ty(ret))`. This
  can be handled with a `Sig::fixed(params: Vec<Ty>, ret: Ty) -> Sig` constructor
  that wraps each `Ty` in `SigTerm::Ty` automatically — so primitive declarations
  don't change syntactically.
- `check/walk.rs` uses `sig.param(i)` and `sig.ret` — these need to become
  `SigTerm`, and callers that previously got a `Ty` now call `resolve(term, σ)`.
- `check/sigs.rs` curated sigs are `Sig::fixed(…)` — unchanged.

## Scoping the first slice (small)

To ship incrementally without touching `NativeFn`:

1. Type variables in *user `sig`/`sig!` declarations only*. Keep `NativeFn::sig`
   as `Sig` with `Vec<Ty>` (introduce `SigTerm` only in the user-sig path).
2. Checkers reads user sigs via `Ctx.declared: HashMap<Symbol, SigWithVars>` (a
   new type alias); primitive sigs stay `Sig`. Merge at the call site: if the
   declared sig has no vars, treat it as a flat `Sig` (existing path unchanged).

This way the blast radius for slice 1 is limited to `annot.rs` (parser),
`check/ctx.rs` (declared map type), and `check/walk.rs` (call-site resolution) —
*not* `NativeFn` and not the hundreds of primitive `Sig::new(…)` calls.

## Interaction with `BROOD_CONTRACTS=1` / `sig!`

Variables in `sig!` pass through silently (unknown types accept). Document this
clearly: a `sig!` with type variables enforces the non-variable parts (shape,
known base types) but not the cross-argument type relationships. For full
cross-argument enforcement you need a handwritten contract:

```lisp
;; sig! enforces fn? and list? but not that they share a type variable
(sig! my-map ((?A -> ?B) (list ?A) -> (list ?B)))

;; handwritten contract enforces element-type coherence (opt-in, expensive):
(defn my-map! (f xs)
  (let (elem-type (if (pair? xs) (type-of (first xs)) 'any))
    ;; ... check f against elem-type ...
    (map f xs)))
```

This is deliberate: a type-variable contract at runtime is costly (you'd have to
inspect element types and cross-reference them), rarely worth it in production,
and not the right tool when `BROOD_CONTRACTS=1` is a dev-time flag.

## When to do this

Gate on a real consumer. Strong signals:
1. A user project (or `std/`) writes enough generic utilities that the loss of
   element type at every user HOF boundary is annoying in practice.
2. The LSP wants to show hover types for a generic function's return — today it
   would show `any`, which is useless.
3. The checker has zero false positives over the full `std/`+`tests/` tree at
   that point (the FP audit baseline is well-established).

Do **not** do this speculatively. `Sig` is the most load-bearing type in the
checker; changing it requires a careful compatibility review.

## Slicing order (once committed)

1. **Grammar + parser** (`annot.rs`): recognise `?A` symbols in `parse_type`,
   return `Ty::ANY` (the runtime path is already correct — unknown → accept).
   The checker ignores vars at this stage (they resolve to `Ty::ANY` everywhere).
   Cost: minimal. Validates the syntax without wiring the inference.
2. **`SigWithVars` in user declarations** (`ctx.rs` + `walk.rs`): introduce
   `SigTerm` and `SigWithVars` for user-declared sigs only; leave `NativeFn::sig`
   unchanged. Wire unification at call sites for user-sig callees. Tests.
3. **Migrate primitives** (optional, later): change `NativeFn::sig` from `Sig` to
   `SigWithVars` and convert every primitive declaration. Unlocks type variables
   in future primitive sigs.
