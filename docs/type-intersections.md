# Type intersections — `(and TypeA TypeB …)` in the type grammar

> Status: **shipped** (both slices). Runtime (`type-matches?`) and static checker
> (`parse_type` → `Ty::intersect`) both handle `(and …)`. See
> `tests/contract_test.blsp` "intersection type checks" for coverage.

## Problem

The current grammar has `(or A B)` for unions but no intersection form. This
limits what annotations can express:

```lisp
;; I want to say: a non-empty list of integers
;; today I can't — I have to fall back to the weaker (list int)
;; and accept that an empty list slips through the runtime check

(sig! process-items ((list int) -> int))
(process-items nil)  ; passes the contract — nil is list?, even though the
                     ; function blows up on an empty input
```

The natural fix is `(and (list int) pair)` — "a list of ints that is
also a cons pair, i.e. non-empty":

```lisp
(sig! process-items ((and (list int) pair) -> int))
(process-items nil)    ; throws — nil is not pair?
(process-items (list 1 2 3)) ; passes — pair? and every element is int
```

Other realistic uses:

```lisp
;; a callable with a known arity — function AND specifically 2-arg
(sig fold-step ((and fn (any any -> any)) any (list any) -> any))

;; a keyword-keyed map (common config-object shape)
(sig configure ((and map (map keyword any)) -> nil))
```

## The algebra is already there

`Ty::intersect` exists in `types/mod.rs` and is already used internally (guard
narrowing, `is_subtype`, `difference`). The gap is purely at the *surface*:

- `parse_type` in `annot.rs` doesn't recognise `(and …)`.
- `type-matches?` in `prelude.blsp` doesn't handle `(and …)`.

No new data type, no new `Ty` variant, no compatibility-contract risk.

## Changes (both slices are small)

### Slice 1 — Brood runtime (`prelude.blsp`)

In the `pair?` branch of `type-matches?`, add a symmetric `and` arm alongside
the existing `or` arm:

```lisp
;; current:
(%eq h 'or) (some? (fn (s) (type-matches? s v)) (rest t))

;; add:
(%eq h 'and) (every? (fn (s) (type-matches? s v)) (rest t))
```

That's the entire runtime change. `every?` already exists; the recursion mirrors
`or` exactly.

### Slice 2 — static checker (`annot.rs`)

In `parse_type`, add an `(and A B …)` case alongside the existing `(or A B …)`
case:

```rust
if value::symbol_is(head, "and") && items.len() >= 2 {
    let mut acc: Option<Ty> = None;
    for &it in &items[1..] {
        let t = parse_type(heap, it)?;
        acc = Some(match acc {
            Some(a) => a.intersect(t),
            None => t,
        });
    }
    return acc;
}
```

The produced `Ty` is semantically correct because `Ty::intersect` is already
well-tested set intersection (`Ty::intersect` = bitwise AND for the flat part;
arrow/elem refinements narrow on match). A `(and int string)` produces
`Ty::NEVER`; the checker's `is_disjoint` test then flags any argument correctly.

### What needs no change

- `types/mod.rs` — `Ty::intersect` is already there.
- `walk.rs` / `guards.rs` — the checker's disjointness path consumes a `Ty`
  from the sig; an intersection `Ty` is just a narrower set, handled identically.
- Grammar description in `type-annotations.md` — add `(and type type+)` to the
  BNF.

## Soundness

The `is_disjoint` check (what fires a warning) is tags-only, and intersection
can only *narrow* the accepted set — it never widens it. So the static checker
becomes *more* precise, never wrong:

- A call that was clean before is still clean (NEVER is impossible to satisfy,
  so NEVER in the expected type means the checker would still fire, correctly).
- An intersection of two non-disjoint types is a valid non-empty set. The checker
  warns when the *argument's* type is disjoint from the intersection — which is
  strictly more precise than the pre-change behaviour.
- Contract side: `(every? …)` is structurally sound — a value passes iff it
  satisfies every constituent.

**One edge case to document:** `(and (list int) (vector string))` is `NEVER` at
the checker level (nil∪pair and vector are disjoint), so any call using this
annotation fires. At runtime, `every?` over an empty conjunction `(and)` returns
`true` (the `(empty? xs) → true` base case in `every?`). We should handle a
bare `(and)` in `parse_type` as `Ty::ANY` to stay consistent.

## Tests to add

In `contract_test.blsp`:

```lisp
;; (and pair (list int)) = non-empty list of ints
(defn c-head (xs) (first xs))
(sig! c-head ((and pair (list int)) -> int))

;; correct: non-empty list of ints
(assert= (c-head (list 1 2 3)) 1)
;; rejected: empty list (nil is not pair?)
(assert-error (c-head nil))
;; rejected: wrong element type
(assert-error (c-head (list "a")))
;; (and int string) is NEVER — nothing can satisfy both
(defn c-never (x) x)
(sig! c-never ((and int string) -> int))
(assert-error (c-never 1))
(assert-error (c-never "a"))
```

Static checker tests (`check.rs`): `(f (and int string))` → NEVER param warns
on any argument; `(f (and int number))` = `int` (intersection subsumed).

## Slicing order

1. **Brood runtime** (prelude.blsp, 1 line). Ship first — immediately useful
   for `sig!` annotations with no Rust build. Add tests in `contract_test.blsp`.
2. **Static checker** (annot.rs, ~10 lines). Adds checker coverage for `sig`
   declarations. Update grammar in `type-annotations.md`.
