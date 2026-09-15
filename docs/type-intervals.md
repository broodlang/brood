# Lengths and indices — intervals in the lattice (ADR-350)

> Status: **shipped** 2026-09-14. The `int` member carries an interval, every countable
> member a length interval; `count` reads it, the sequence builders move it, a comparison
> guard narrows it, and a positional read is present when the length says so. The grammar
> spells them `(int lo hi)` and `(len T lo hi)`.

## Problem

The largest class left in `tests/`'s strict findings, and bedit's, was `nil | int` from a
read the code had just made safe: `(nth words 1)` after `(>= n 4)` with `n` the count,
`(first ms)` after `(not (empty? ms))`, `(nth parts 1)` after `(= (count parts) 3)`. Each is
a fact about a LENGTH, stated in the code, that the lattice had no slot for — so `nth`
answered `elem | nil` every time and the honest fix at the call site was an `(or … default)`
that the runtime never took. Idris's `Vect n a` says the same thing in a dependent type;
this lattice says it as a refinement, which is the clothes it already wears.

## Representation

- `Range { lo: Option<i64>, hi: Option<i64> }` — a closed interval, either end open. Hull,
  meet (`None` when empty), inclusion, disjointness, and the four arithmetic operations on
  intervals with *checked* arithmetic — an overflow makes the end unbounded, never wrong.
- `int_range: Option<Range>` on a term's `int` member. Canonical: absent when a positive
  literal set is present (the set is the sharper statement — `{3, 5}` needs no `[3..5]`),
  absent when unbounded. `Ty::int_in(r)` builds one; `int_range_eff` reads the effective
  interval (the hull of a literal set, the slot, or everything).
- `len: Option<Range>` on a term's countable members — `pair`, `vector`, `set`, `map`,
  `string`, `bytes` (`COUNT_BITS`). `nil` is never in it: a list of length 0 IS `nil`, so
  `with_len` drops the `nil` tag when 0 is excluded and the `pair` tag when the length is
  exactly 0. Canonical: `lo` clamped at 0, `[0..]` absent. A positional shape (`tuple`,
  `list_shape`) IS its arity and stores no `len` (`normalise_len`); a `pair` with no slot is
  at least 1 (`len_eff`).
- Display: `int[0..]`, `vector<int>[3]`, `list<string>[1..]`, `string[..5]`. Source:
  `(int 0 _)`, `(len (vector int) 3 3)`, `(len (list string) 1 _)` — `_` is the open end.
  `(list E)` in the grammar is now `nil | list<E>` (a list that may be empty), matching
  what every list-returning function produces; the non-empty list is `(len (list E) 1 _)`.

## The set relations

Unions **hull** (never contested — the merge stays exact where it was); intersections
**meet**, an empty meet dropping the tag (`int[0..5] ∩ int[10..]` has no int); subtyping is
interval inclusion; disjointness by interval when the only shared tag is `int` or only
countable tags are shared. `is_known_only_by_exclusion` is asked of the type WITHOUT its
intervals — a length is a positive fact, not an exclusion, and `¬(nil | false)` with a
length is still known only by what it excludes.

## Inference

- `count` / `string/length` / `vector-length` → `int` in the collection's length.
- `first` / `second` / `third` / `last` / `nth`: present (no `nil`) when the length proves
  it — `second` of a `[2..]`, `(nth xs 3)` of a `[4..]`, `(nth xs i)` when `i ≥ 0` and
  either a guard established `i < (count xs)` (`Ctx::index_bounds`) or `i`'s own interval
  ends below the length. A read from exactly `nil` is `nil` (or the `nth` default).
- `rest` / `but-last` shorten the length by one; `cons` / `conj` lengthen it; `range` takes
  its length from its bound; `seq`, `reverse`, `sort`, `map`, … carry it through
  (`list_with_len`); `(list …)` and a quoted list are their arity.
- Int-closed arithmetic carries intervals: `+ - * inc dec abs mod rem quot` (and the
  `math/` spellings). `apply` and the one-step numeric fold drop them — `(apply + [1 2])`
  is not `int[1..2]`.
- `bytes` elements are `int[0..255]`.

### Widening

An ascent of intervals never converges on its own — a counter reads `0`, then `0 | 1`,
then `[0..2]`, … — so every fixpoint widens: an interval end that moved outward between
rounds goes to its infinity (`widen_intervals_against`), BEFORE the recursive fold of
ADR-349 is tried. Same-tag alternatives are merged first (nil aside; `tagged_apart` tuples
kept separate), and a literal set that grew is widened to its tag's interval hull for the
same reason. The three sites: Pass 2.9's parameter and return loops, `specialize_recursive`,
and the fold-accumulator loop.

## Guards

- `(empty? xs)` is biconditional by length: true is `nil` or a countable of length 0;
  false is a countable of length at least 1 (`Guard.else_ty` — what a falsy test proves,
  stated positively, when that is sharper than `¬ty`).
- A comparison between two sides — a local, `(count local)`, an int literal —
  (`comparison_facts`) narrows each side's interval in both branches (`<`, `<=`, `>`, `>=`,
  `=`, and their `not`), and `(< i (count xs))` records the index bound `i < |xs|` for the
  positional reads in the then-branch. A count alias `(let (n (count xs)) …)` narrows
  `xs`'s length whenever `n` is narrowed (`Ctx::count_aliases`).
- `(= (nth a k) lit)` is a path guard on position `k` (biconditional for an exact literal),
  and `Ctx::narrow_path` on an index path drops the positional alternatives of the base
  whose element there cannot be what the guard established — in BOTH branches. This is
  the `[:ok x] | [:error msg]` dispatch: `(if (= (nth r 0) :error) r (nth r 1))` types the
  whole `r` in the then-branch and the `:ok` payload in the else.

## Where the fixpoints read their branches

`specialize_recursive`'s self-call sites are typed in the `if` branch they sit in
(`branch_scopes`, the same scoping inference reads the branches under), and a branch the
test has killed contributes no site — so a list walk's accumulator takes an element of the
list, not an element-or-nil, because the self-call is in the else of `(nil? xs)`. This
was the missing half of `path/join`'s return type.

## Soundness

Every interval is an over-approximation of the values a term admits, and every operation
keeps it one: hull on union, meet on intersection, checked arithmetic that widens to
unbounded rather than wrap, a length that never claims what the shape does not carry, and
a widening that only moves an end outward. A read is pronounced present only from a lower
bound on the length that the lattice holds, never from the absence of one.

## Deferred

- A relation between two locals beyond `i < |xs|` (`i < j`, `i + 1 ≤ |xs|`): the index
  bound is the case the corpus asks for; a general relational domain is a different lattice.
- A bound stated in the CALLER: `regex-run-anchored-loop` reads `(nth codes i)` under
  `(= i n)` where `n = (count codes)` was computed one frame up, so the two
  `(check-allow :type-mismatch …)` scopes in `std/regex.blsp` stay — the fact would have to
  travel through the call as a relation between two parameters.
- A `float` interval: nothing in the corpus reads one.
- The runtime contract (`BROOD_CONTRACTS`) reads the interval grammar and checks the
  tag; checking the bound at runtime is a separate decision.
